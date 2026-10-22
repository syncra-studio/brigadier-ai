//! A session thread's scratch goes once its requests are over ([`super::super::thread_scratch`]):
//! what its commands left in its temporary folder, and the processes working there, but a
//! server that listens on a port. A request still waiting keeps it, no turn starts while it
//! goes, and the launch sweeps what a quit or a crash left.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::workers::session_temp_dir;
use crate::work::RequestState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A process of the thread's commands working in `dir`, in a group of its own, as a test the
/// thread started and killed leaves its children.
fn spawn_in(dir: &Path, program: &str, args: &[&str]) -> Child {
    use std::os::unix::process::CommandExt;
    Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap()
}

/// What the thread's turn leaves in its temporary folder: a second checkout's build output, a
/// stopped test's process and a running helper's.
fn litter(turn: &Turn, children: &Arc<Mutex<Vec<Child>>>) -> PathBuf {
    let tmp = session_temp_dir(&turn.conversation().unwrap());
    let target = tmp.join("bt-main/target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("out.bin"), vec![7u8; 64 * 1024]).unwrap();
    let stopped = spawn_in(&target, "/bin/sleep", &["300"]);
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(stopped.id() as i32),
        nix::sys::signal::Signal::SIGSTOP,
    )
    .unwrap();
    let running = spawn_in(&tmp, "/bin/sleep", &["300"]);
    children.lock().unwrap().extend([stopped, running]);
    tmp
}

/// Whether every child has ended (and is reaped).
fn all_ended(children: &Arc<Mutex<Vec<Child>>>) -> bool {
    children
        .lock()
        .unwrap()
        .iter_mut()
        .all(|child| child.try_wait().ok().flatten().is_some())
}

/// Ends what a failed test left running.
fn end(children: &Arc<Mutex<Vec<Child>>>) {
    for child in children.lock().unwrap().iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Once its request is over, what the thread's commands left in its temporary folder goes,
/// stopped processes included; the folder stays, as its CLI's TMPDIR.
#[tokio::test]
async fn a_threads_scratch_goes_once_its_request_is_over() {
    let children: Arc<Mutex<Vec<Child>>> = Arc::default();
    let made = children.clone();
    let flow = Flow::start(
        "scratch-over",
        Options::default(),
        script(move |turn| {
            let made = made.clone();
            async move {
                litter(&turn, &made);
                Reply::text("Built it in a second checkout.")
            }
        }),
    )
    .await;
    flow.say("Build main apart.").await;
    flow.settled().await;
    let tmp = session_temp_dir(&flow.conversation);
    super::eventually("the scratch to go", || {
        std::fs::read_dir(&tmp).is_ok_and(|mut entries| entries.next().is_none())
    })
    .await;
    super::eventually("its processes to end", || all_ended(&children)).await;
    assert!(tmp.is_dir(), "{}", tmp.display());
    end(&children);
    flow.stop().await;
}

/// A request waiting on the user's answer keeps the thread's scratch; once it is over, it goes.
#[tokio::test]
async fn a_waiting_request_keeps_the_threads_scratch() {
    let children: Arc<Mutex<Vec<Child>>> = Arc::default();
    let made = children.clone();
    let flow = Flow::start(
        "scratch-waiting",
        Options::default(),
        script(move |turn| {
            let made = made.clone();
            async move {
                if turn.input.contains("[answer]") {
                    return Reply::text("Kept the CSV export.");
                }
                litter(&turn, &made);
                let asked = turn
                    .call(
                        "ask_user",
                        json!({ "questions": [{ "question": "Which format?",
                            "options": [{ "label": "CSV" }, { "label": "JSON" }],
                            "recommended": 0 }] }),
                    )
                    .await;
                assert!(!asked.is_error, "{}", asked.text);
                Reply::text("")
            }
        }),
    )
    .await;
    flow.say("Plan the export.").await;
    let board = flow
        .until("the request to wait on its card", |board| {
            board
                .latest_request()
                .is_some_and(|request| request.state == RequestState::Waiting)
        })
        .await;
    flow.settled().await;
    let tmp = session_temp_dir(&flow.conversation);
    // Give a sweep the time it would take: nothing goes.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(tmp.join("bt-main/target/out.bin").exists());
    assert!(
        children
            .lock()
            .unwrap()
            .iter_mut()
            .all(|child| child.try_wait().ok().flatten().is_none())
    );
    let card = board
        .questions
        .values()
        .find(|question| question.is_open())
        .unwrap()
        .id
        .clone();
    flow.manager
        .answer_question(flow.conversation.clone(), card, vec!["CSV".into()], None)
        .await
        .unwrap();
    super::eventually("the scratch to go", || !tmp.join("bt-main").exists()).await;
    super::eventually("its processes to end", || all_ended(&children)).await;
    end(&children);
    flow.stop().await;
}

/// A server the thread's commands left listening on a port stays, and so does the folder it
/// works in; what is stopped beside it goes.
#[tokio::test]
async fn a_listening_server_stays_through_the_sweep() {
    let children: Arc<Mutex<Vec<Child>>> = Arc::default();
    let made = children.clone();
    let flow = Flow::start(
        "scratch-server",
        Options::default(),
        script(move |turn| {
            let made = made.clone();
            async move {
                let tmp = litter(&turn, &made);
                let site = tmp.join("site");
                std::fs::create_dir_all(&site).unwrap();
                let server = spawn_in(
                    &site,
                    "python3",
                    &[
                        "-c",
                        "import socket, time\ns = socket.socket()\ns.bind(('127.0.0.1', 0))\ns.listen()\nopen('up', 'w').close()\ntime.sleep(300)",
                    ],
                );
                made.lock().unwrap().push(server);
                for _ in 0..250 {
                    if site.join("up").exists() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                assert!(site.join("up").exists());
                Reply::text("The site serves.")
            }
        }),
    )
    .await;
    flow.say("Serve the site.").await;
    flow.settled().await;
    let tmp = session_temp_dir(&flow.conversation);
    super::eventually("the build output to go", || !tmp.join("bt-main").exists()).await;
    let ended = |index: usize| {
        children.lock().unwrap()[index]
            .try_wait()
            .ok()
            .flatten()
            .is_some()
    };
    super::eventually("the stopped and idle processes to end", || {
        ended(0) && ended(1)
    })
    .await;
    assert!(!ended(2), "the server was ended");
    assert!(tmp.join("site/up").exists());
    end(&children);
    flow.stop().await;
}

/// While the scratch is reserved for a sweep, a message the user sends starts no turn; once
/// the sweep is done, it does.
#[tokio::test]
async fn no_turn_starts_while_the_scratch_is_swept() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "scratch-race",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the thread to be free", || conv.begin_scratch_sweep()).await;
    flow.say("Second.").await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        inputs.lock().unwrap().len(),
        1,
        "a turn ran during the sweep"
    );
    conv.end_scratch_sweep().await;
    flow.manager.kick(&conv);
    flow.until("the second turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    assert!(inputs.lock().unwrap()[1].contains("Second."));
    flow.stop().await;
}

/// What a quit left in a thread's temporary folder goes when the app starts again, before
/// anything runs.
#[tokio::test]
async fn the_launch_sweeps_what_a_quit_left() {
    let mut flow = Flow::start(
        "scratch-launch",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let tmp = session_temp_dir(&flow.conversation);
    super::scratch_sweep_over(&flow, &tmp).await;
    // Left while the app was down.
    std::fs::create_dir_all(tmp.join("bt-verify/target")).unwrap();
    std::fs::write(tmp.join("bt-verify/target/out.bin"), b"built").unwrap();
    let children: Arc<Mutex<Vec<Child>>> =
        Arc::new(Mutex::new(vec![spawn_in(&tmp, "/bin/sleep", &["300"])]));
    flow.restart().await;
    assert!(!tmp.join("bt-verify").exists());
    super::eventually("the leftover process to end", || all_ended(&children)).await;
    assert!(tmp.is_dir());
    end(&children);
    flow.stop().await;
}

/// A server started through a wrapper (`sh -c`, as `npm run dev` starts node) in the temporary
/// folder itself stays with its wrapper, and so does everything in the folder it works in.
#[tokio::test]
async fn a_server_below_a_wrapper_in_the_folder_itself_keeps_it() {
    let children: Arc<Mutex<Vec<Child>>> = Arc::default();
    let made = children.clone();
    let flow = Flow::start(
        "scratch-wrapper",
        Options::default(),
        script(move |turn| {
            let made = made.clone();
            async move {
                let tmp = session_temp_dir(&turn.conversation().unwrap());
                std::fs::create_dir_all(&tmp).unwrap();
                std::fs::write(tmp.join("index.html"), b"<p>served</p>").unwrap();
                let wrapper = spawn_in(
                    &tmp,
                    "/bin/sh",
                    &[
                        "-c",
                        "python3 -c \"import socket, time\ns = socket.socket()\ns.bind(('127.0.0.1', 0))\ns.listen()\nopen('up', 'w').close()\ntime.sleep(300)\"; true",
                    ],
                );
                made.lock().unwrap().push(wrapper);
                for _ in 0..250 {
                    if tmp.join("up").exists() {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                assert!(tmp.join("up").exists());
                Reply::text("The site serves.")
            }
        }),
    )
    .await;
    flow.say("Serve the site.").await;
    flow.settled().await;
    let tmp = session_temp_dir(&flow.conversation);
    // The sweep the request's end started may still hold the thread: sweep again once free.
    let mut swept = None;
    for _ in 0..250 {
        swept = flow
            .manager
            .sweep_thread_scratch(&flow.conversation)
            .await
            .unwrap();
        if swept.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let swept = swept.expect("the session to be idle");
    assert!(
        swept
            .kept
            .iter()
            .any(|kept| kept.contains("listens on a port")),
        "{swept:?}"
    );
    let wrapper = children.lock().unwrap()[0].id();
    let alive =
        |pid: u32| nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok();
    assert!(alive(wrapper), "the wrapper was ended: {swept:?}");
    assert!(tmp.join("index.html").exists(), "{swept:?}");
    assert!(tmp.join("up").exists(), "{swept:?}");
    // The wrapper's group holds the server too.
    let group = nix::unistd::Pid::from_raw(-(wrapper as i32));
    let _ = nix::sys::signal::kill(group, nix::sys::signal::Signal::SIGKILL);
    end(&children);
    let _ = std::fs::remove_file(tmp.join("index.html"));
    let _ = std::fs::remove_file(tmp.join("up"));
    flow.stop().await;
}

/// A temporary folder that became a link is left alone, and so is what it points to and what
/// works there: the sweep would otherwise delete and end what isn't the thread's.
#[tokio::test]
async fn a_temp_folder_replaced_by_a_link_is_left_alone() {
    let flow = Flow::start(
        "scratch-link",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    let tmp = session_temp_dir(&flow.conversation);
    super::scratch_sweep_over(&flow, &tmp).await;
    let elsewhere =
        std::env::temp_dir().join(format!("brigadier-scratch-link-{}", flow.conversation));
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("keep.txt"), b"not the thread's").unwrap();
    std::fs::remove_dir(&tmp).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &tmp).unwrap();
    let children: Arc<Mutex<Vec<Child>>> = Arc::new(Mutex::new(vec![spawn_in(
        &elsewhere,
        "/bin/sleep",
        &["300"],
    )]));
    flow.manager.sweep_scratch_at_launch().await;
    let swept = flow
        .manager
        .sweep_thread_scratch(&flow.conversation)
        .await
        .unwrap();
    let kept = elsewhere.join("keep.txt").exists();
    let running = children.lock().unwrap()[0].try_wait().unwrap().is_none();
    end(&children);
    let _ = std::fs::remove_file(&tmp);
    let _ = std::fs::remove_dir_all(&elsewhere);
    assert!(kept, "a file the link points to was deleted: {swept:?}");
    assert!(
        running,
        "a process where the link points was ended: {swept:?}"
    );
    flow.stop().await;
}
