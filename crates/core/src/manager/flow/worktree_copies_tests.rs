//! A new tab can run the app: its worktree gets copies of the checkout's gitignored `.env*`
//! files and of what `.worktreeinclude` names, and the thread hears so once. A tracked
//! `.brigadier/setup` is run by the engine, once, before the first turn (or a worker's first
//! command), under the session's access and only in a folder the user trusts; the session
//! hears how it went (#326).

use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script};
use crate::manager::prompts::SETUP_LINE;
use crate::model::PermissionLevel;

const FILES: &[(&str, &str)] = &[
    (
        ".gitignore",
        ".env*\n!.env.example\n*.log\nconfig/local.json\n",
    ),
    (".env.example", "API_KEY=\n"),
    (".env", "API_KEY=flow-secret-value-123\n"),
    ("apps/web/.env.local", "PORT=4321\n"),
    (".worktreeinclude", "config/local.json\n"),
    ("config/local.json", "{\"debug\": true}\n"),
    (
        ".brigadier/setup",
        "#!/bin/sh\necho installing the tools\ntouch .setup-ran\n",
    ),
];

/// What the thread or a worker hears once its new worktree's setup script ran fine.
const RAN: &str = "Brigadier ran this repository's setup script `.brigadier/setup` in your new worktree before you started (exit 0); don't run it again. The end of its output:\n```\ninstalling the tools\n```";

/// The first turn's input and whether its worktree had each copy (and the setup script's
/// mark).
type Seen = Arc<Mutex<Vec<(String, Vec<bool>)>>>;

async fn first_turns(name: &str, trusted: bool) -> Vec<(String, Vec<bool>)> {
    first_turns_with(name, trusted, FILES.to_vec(), PermissionLevel::Full).await
}

async fn first_turns_with(
    name: &str,
    trusted: bool,
    files: Vec<(&'static str, &'static str)>,
    permission: PermissionLevel,
) -> Vec<(String, Vec<bool>)> {
    let seen: Seen = Arc::default();
    let script: Script = {
        let seen = seen.clone();
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                let has = [
                    ".env",
                    "apps/web/.env.local",
                    "config/local.json",
                    ".setup-ran",
                ]
                .map(|file| {
                    turn.cwd.join(file).is_file()
                        || turn.add_dirs.iter().any(|dir| dir.join(file).is_file())
                })
                .to_vec();
                seen.lock().unwrap().push((turn.input.clone(), has));
                Reply::text("Done.")
            })
        })
    };
    let options = Options {
        files,
        trusted: Some(trusted),
        permission,
        ..Options::default()
    };
    let flow = Flow::start(name, options, script).await;
    flow.say("Start the app").await;
    flow.settled().await;
    flow.say("And again").await;
    flow.settled().await;
    // The checkout's own files are where they were, and none was committed.
    assert!(flow.repo.join(".env").is_file());
    let tracked = super::git(&flow.repo, &["ls-files"]);
    assert!(
        !tracked
            .lines()
            .any(|file| file == ".env" || file == "config/local.json"),
        "{tracked}"
    );
    flow.stop().await;
    seen.lock().unwrap().clone()
}

#[tokio::test]
async fn a_new_tab_gets_the_checkouts_env_files_and_its_setup_script_run() {
    let seen = first_turns("worktree-copies", true).await;
    assert_eq!(seen.len(), 2, "{seen:?}");
    let (input, has) = &seen[0];
    // The script ran before the first turn, and the thread hears how it went.
    assert_eq!(has, &[true, true, true, true], "{input}");
    assert!(
        input.contains("[setup] Your worktree has copies of the user's gitignored files from their checkout, so the app runs here as it does there: `.env`, `apps/web/.env.local`, `config/local.json`."),
        "{input}"
    );
    assert!(input.contains(&format!("[setup] {RAN}")), "{input}");
    assert!(!input.contains(SETUP_LINE), "{input}");
    // Once: the next message's worktree is the same one.
    assert!(!seen[1].0.contains("[setup]"), "{}", seen[1].0);
}

#[tokio::test]
async fn an_untrusted_folder_gets_the_copies_but_no_setup_script() {
    let seen = first_turns("worktree-copies-untrusted", false).await;
    let (input, has) = &seen[0];
    assert_eq!(has, &[true, true, true, false], "{input}");
    assert!(
        input.contains("[setup] Your worktree has copies"),
        "{input}"
    );
    assert!(!input.contains(SETUP_LINE), "{input}");
    assert!(!input.contains(".brigadier/setup"), "{input}");
}

/// A failing script is a fact the thread hears, with the end of its output; the turn goes on.
#[tokio::test]
async fn a_failing_setup_script_is_told_not_held() {
    let mut files = FILES.to_vec();
    files.retain(|(path, _)| *path != ".brigadier/setup");
    files.push((
        ".brigadier/setup",
        "#!/bin/sh\necho fetching\necho no network >&2\nexit 3\n",
    ));
    let seen = first_turns_with(
        "worktree-copies-failing",
        true,
        files,
        PermissionLevel::Full,
    )
    .await;
    let (input, _) = &seen[0];
    assert!(
        input.contains("[setup] Brigadier ran this repository's setup script `.brigadier/setup` in your new worktree before you started, and it failed (exit 3). The worktree is otherwise ready: deal with what the failure means for your work. The end of its output:\n```\nfetching\nno network\n```"),
        "{input}"
    );
}

/// Below Full access the script runs in the thread's sandbox, which writes its worktree.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_sandboxed_thread_has_its_setup_script_run_in_its_sandbox() {
    let seen = first_turns_with(
        "worktree-copies-sandboxed",
        true,
        FILES.to_vec(),
        PermissionLevel::AcceptEdits,
    )
    .await;
    let (input, has) = &seen[0];
    assert_eq!(has, &[true, true, true, true], "{input}");
    assert!(input.contains(&format!("[setup] {RAN}")), "{input}");
}

/// The user's Stop ends a setup script still running, as it ends the thread's commands.
#[cfg(unix)]
#[tokio::test]
async fn the_users_stop_ends_a_setup_script_still_running() {
    let mut files = FILES.to_vec();
    files.retain(|(path, _)| *path != ".brigadier/setup");
    files.push((
        ".brigadier/setup",
        "#!/bin/sh\ntouch .setup-started\nsleep 60\ntouch .setup-ran\n",
    ));
    let options = Options {
        files,
        ..Options::default()
    };
    let flow = Flow::start(
        "worktree-copies-stopped",
        options,
        // A real turn takes a while: the Stop replayed at its start reaches it.
        Arc::new(|turn| {
            Box::pin(async move {
                assert!(turn.stopped().await, "the turn the user stopped is stopped");
                Reply::text("Done.")
            })
        }),
    )
    .await;
    let conversation = flow.core.conversation(&flow.conversation).unwrap();
    let worktree = flow.manager.session_worktree_path(&conversation);
    let started = std::time::Instant::now();
    let stop = async {
        while !worktree.join(".setup-started").is_file() {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        flow.manager
            .interrupt(flow.conversation.clone())
            .await
            .unwrap();
    };
    tokio::join!(flow.say("Start the app"), stop);
    let board = flow.settled().await;
    // The request the user stopped as it started ends stopped.
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == crate::work::RequestState::Stopped),
        "{:?}",
        board.requests
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "{:?}",
        started.elapsed()
    );
    assert!(!worktree.join(".setup-ran").exists());
    flow.stop().await;
}
