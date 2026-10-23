//! Lossless trimming of the thread's command output (THREAD-PLAN.md Q4): the Claude hook's
//! grant and what it stores, an `out-<id>` alias and the file it names (its owner only, paged
//! back byte for byte, gone with the conversation), and Brigadier's own command runner.

use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::model::{Origin, SessionSpec};

use super::{Flow, Options, Reply, Scratch, Script, Turn};
use crate::manager::HOOK_GRANT_ENV;

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
    let grant = spec
        .env
        .iter()
        .find(|(key, _)| key == brigadier_cli::protocol::GRANT_ENV)
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
        .split("[full output: ")
        .nth(1)
        .and_then(|rest| rest.split([',', ' ']).next())
        .expect("an alias")
        .to_owned()
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
