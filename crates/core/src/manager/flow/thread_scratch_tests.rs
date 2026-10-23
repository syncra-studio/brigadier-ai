//! A session's TMPDIR survives turns and restarts; Archive and Delete own its cleanup.

use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, Turn};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Kill and reap only the process this test started, including on assertion failure.
#[derive(Default)]
struct Background(Option<Child>);

impl Background {
    #[track_caller]
    fn assert_running(&mut self) {
        assert!(
            self.0
                .as_mut()
                .expect("the session started sleep")
                .try_wait()
                .unwrap()
                .is_none(),
            "the session's background process was ended"
        );
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[tokio::test]
async fn tmp_and_background_process_survive_turn_end_and_daemon_restart() {
    let background = Arc::new(Mutex::new(Background::default()));
    let started = background.clone();
    let mut flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w99-tmp-survives",
        Options::default(),
        script(move |turn| {
            let started = started.clone();
            async move {
                let tmp = turn.session_tmp();
                let mut background = started.lock().unwrap();
                if background.0.is_none() {
                    std::fs::write(tmp.join("keep.txt"), b"keep between turns").unwrap();
                    use std::os::unix::process::CommandExt;
                    background.0 = Some(
                        Command::new("/bin/sleep")
                            .arg("600")
                            .current_dir(&tmp)
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .process_group(0)
                            .spawn()
                            .unwrap(),
                    );
                } else {
                    assert_eq!(
                        std::fs::read(tmp.join("keep.txt")).unwrap(),
                        b"keep between turns"
                    );
                    background.assert_running();
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Leave a file and a background sleep in TMPDIR.")
        .await;
    flow.idle().await;
    let file = flow
        .manager
        .session_dir(&flow.conversation)
        .tmp()
        .join("keep.txt");
    assert_eq!(std::fs::read(&file).unwrap(), b"keep between turns");
    background.lock().unwrap().assert_running();

    flow.say("Check them on the next turn.").await;
    flow.idle().await;
    flow.restart().await;
    assert_eq!(std::fs::read(&file).unwrap(), b"keep between turns");
    background.lock().unwrap().assert_running();
    flow.say("Check them after the restart.").await;
    flow.idle().await;
    assert_eq!(std::fs::read(&file).unwrap(), b"keep between turns");
    background.lock().unwrap().assert_running();
    flow.stop().await;
}

#[tokio::test]
async fn archive_removes_the_tmp_files_a_turn_left() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w99-tmp-archive",
        Options::default(),
        script(|turn| async move {
            let output = turn.session_tmp().join("checkout/target");
            std::fs::create_dir_all(&output).unwrap();
            std::fs::write(output.join("out.bin"), b"built").unwrap();
            Reply::text("Built.")
        }),
    )
    .await;
    flow.say("Build in TMPDIR.").await;
    flow.idle().await;
    let id = flow.conversation.clone();
    let dir = flow.manager.session_dir(&id);
    assert!(dir.tmp().join("checkout/target/out.bin").is_file());
    flow.manager.archive(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    super::eventually_async("the archive's cleanup to finish", || async {
        !flow.core.conversation(&id).unwrap().cleanup_pending
    })
    .await;
    assert!(!dir.tmp().exists());
    assert!(dir.record().is_dir());
    flow.stop().await;
}
