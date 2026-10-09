//! A flow test leaves nothing behind however it ends: its session's folder and its tasks'
//! test data folders are gone once its thread is, whether it stopped the session, never did,
//! failed part-way, or failed while the session was starting.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use serde_json::json;

use super::{Flow, Options, Reply, Scratch, Script, Turn};
use crate::manager::workers::test_data_dir;
use crate::work::PhaseStage;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Runs `test` as a test runs: on a thread of its own with a fresh runtime. Returns whether
/// it passed and the folders it sent while it ran, once its thread has ended.
fn run<F, Fut>(test: F) -> (bool, Vec<PathBuf>)
where
    F: FnOnce(Sender<PathBuf>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    let (sent, folders) = std::sync::mpsc::channel();
    let passed = std::thread::Builder::new()
        .name("a flow test".into())
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(test(sent));
        })
        .unwrap()
        .join()
        .is_ok();
    (passed, folders.try_iter().collect())
}

/// A session whose lead waits for the go-ahead on its outline: a live task, so its test data
/// folder is there. Sends the session's folder and that one.
async fn waiting_lead(name: &str, sent: &Sender<PathBuf>) -> Flow {
    let flow = Flow::start(
        name,
        Options {
            plan_mode: true,
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                if !turn.input.contains("[outline") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Add a file", "kind": "implement",
                                   "spec": "Create one.txt.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            let reply = turn
                .call("submit_outline", json!({"outline": "1. Create one.txt"}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Waiting for the go-ahead.")
        }),
    )
    .await;
    flow.say("Add one.txt.").await;
    let board = flow
        .until("the outline waits for the user", |board| {
            board.plans.values().any(|plan| {
                plan.steps
                    .iter()
                    .any(|step| step.stage == PhaseStage::AwaitingGoAhead)
            })
        })
        .await;
    let folder = test_data_dir(&Flow::task(&board, 1).id);
    assert!(folder.is_dir());
    sent.send(flow.dir.to_path_buf()).unwrap();
    sent.send(folder).unwrap();
    flow
}

fn assert_gone(folders: &[PathBuf]) {
    assert_eq!(folders.len(), 2, "{folders:?}");
    for folder in folders {
        assert!(!folder.exists(), "{} is left", folder.display());
    }
}

#[test]
fn a_flow_test_that_panics_leaves_no_folders() {
    let (passed, folders) = run(|sent| async move {
        let _flow = waiting_lead("litter-panics", &sent).await;
        panic!("the test fails with its task still waiting");
    });
    assert!(!passed);
    assert_gone(&folders);
}

#[test]
fn a_flow_test_that_never_stops_its_session_leaves_no_folders() {
    let (passed, folders) = run(|sent| async move {
        waiting_lead("litter-unstopped", &sent).await;
    });
    assert!(passed);
    assert_gone(&folders);
}

#[test]
fn a_flow_test_that_stops_its_session_leaves_no_folders() {
    let (passed, folders) = run(|sent| async move {
        waiting_lead("litter-stopped", &sent).await.stop().await;
    });
    assert!(passed);
    assert_gone(&folders);
}

/// A session that fails while it starts (here its store copy is missing, after its
/// repository was made) leaves no folder either.
#[test]
fn a_flow_that_fails_while_starting_leaves_no_folder() {
    let name = format!("litter-start-{}", uuid::Uuid::new_v4().simple());
    let prefix = format!("brigadier-flow-{name}-");
    let (passed, _) = run(move |_| async move {
        Flow::start(
            &name,
            Options {
                store: Some(PathBuf::from("/nonexistent/brigadier.db")),
                ..Options::default()
            },
            script(|_| async { Reply::text("Done.") }),
        )
        .await;
    });
    assert!(!passed);
    let left: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
        .map(|entry| entry.path())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

/// Work still in flight when a session is dropped may write in its folder afterwards: the
/// folder is removed again as the test's thread ends.
#[test]
fn a_write_after_the_drop_is_removed_as_the_thread_ends() {
    let (passed, folders) = run(|sent| async move {
        let scratch = Scratch::new("litter-late");
        let late = scratch.join("data/worktrees/project");
        sent.send(scratch.to_path_buf()).unwrap();
        drop(scratch);
        std::fs::create_dir_all(&late).unwrap();
    });
    assert!(passed);
    assert!(!folders[0].exists(), "{} is left", folders[0].display());
}

/// A test process killed before it dropped its folders leaves them to the next one, which
/// ends what still runs in them and removes them; those of a process still running stay.
#[cfg(unix)]
#[test]
fn the_folders_of_a_killed_test_process_are_removed() {
    let temp = std::env::temp_dir();
    let mut running = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    let mut ended = std::process::Command::new("true").spawn().unwrap();
    ended.wait().unwrap();
    let folder = |pid: u32| {
        let folder = temp.join(format!(
            "brigadier-flow-litter-killed-{pid}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(folder.join("data")).unwrap();
        folder
    };
    let killed = folder(ended.id());
    // A preview the killed process left running in it.
    let mut orphan = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&killed)
        .spawn()
        .unwrap();
    let alive = folder(running.id());
    let ours = folder(std::process::id());
    let unnamed = temp.join(format!("brigadier-flow-litter-killed-{}", ended.id()));
    std::fs::create_dir_all(&unnamed).unwrap();
    super::remove_killed_tests(&temp);
    let orphan_ended = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            while orphan.try_wait().unwrap().is_none() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            orphan.try_wait().unwrap().is_some()
        });
    let kept = [alive.exists(), ours.exists(), unnamed.exists()];
    running.kill().unwrap();
    running.wait().unwrap();
    for folder in [&alive, &ours, &unnamed] {
        std::fs::remove_dir_all(folder).unwrap();
    }
    if !orphan_ended {
        orphan.kill().unwrap();
        orphan.wait().unwrap();
    }
    assert!(orphan_ended, "what ran in it is ended");
    assert!(!killed.exists());
    assert_eq!(kept, [true; 3]);
}
