//! The session's own worktree gets warm (BUILD-PLAN C7): the thread works in it as a worker
//! does in its own, so it gets the same copies of the checkout's dependency installs, and
//! hears so, instead of failing its first check and reinstalling them. The copies come behind
//! its first turn, which never waits for them.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use brigadier_sandbox::clone::Folder;

use super::{Flow, Options, Reply, Script};

/// Dates `node_modules` and what is at its top a minute back, so it doesn't look mid-install.
fn age(dir: &std::path::Path) {
    let past = SystemTime::now() - Duration::from_secs(60);
    let mut paths = vec![dir.to_owned()];
    paths.extend(
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.path()),
    );
    for path in paths {
        std::fs::File::open(path)
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
}

/// Whether a copy of the checkout can be made next to it: copies are only ever made
/// copy-on-write, which a file system that can't share blocks (ext4) refuses, and then
/// nothing is copied.
fn copies_here(repo: &Path) -> bool {
    let parent = repo.parent().unwrap();
    let probe = parent.join("clone-check");
    let made = Folder::open(repo, Path::new(""), false)
        .and_then(|src| {
            Folder::open(parent, Path::new(""), false)?.clone_in(
                &src,
                OsStr::new("package.json"),
                OsStr::new("clone-check"),
                Instant::now() + Duration::from_secs(30),
            )
        })
        .is_ok();
    let _ = std::fs::remove_file(probe);
    made
}

#[tokio::test]
async fn the_threads_worktree_gets_the_checkouts_dependencies_and_the_thread_is_told() {
    let seen: Arc<Mutex<Vec<(String, std::path::PathBuf)>>> = Arc::default();
    let script: Script = {
        let seen = seen.clone();
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.lock()
                    .unwrap()
                    .push((turn.input.clone(), turn.cwd.clone()));
                Reply::text("Done.")
            })
        })
    };
    let options = Options {
        files: vec![
            (".gitignore", "node_modules/\n"),
            (
                "package.json",
                "{\"name\": \"flow\", \"version\": \"1.0.0\"}\n",
            ),
            ("package-lock.json", "{\"lockfileVersion\": 3}\n"),
            ("node_modules/left-pad/index.js", "module.exports = 1;\n"),
        ],
        ..Options::default()
    };
    let flow = Flow::start("warm-session", options, script).await;
    if !copies_here(&flow.repo) {
        flow.stop().await;
        return;
    }
    age(&flow.repo.join("node_modules"));
    flow.say("Run the tests").await;
    flow.settled().await;
    let worktree = seen.lock().unwrap()[0].1.clone();
    super::eventually("the worktree to get node_modules", || {
        worktree.join("node_modules/left-pad/index.js").is_file()
    })
    .await;
    // The thread hears it with the message after the copy came in (the first one, if it came
    // that soon), and only once: the next message's worktree is the same one.
    flow.say("And again").await;
    flow.settled().await;
    flow.say("Once more").await;
    flow.settled().await;
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    let told: Vec<_> = seen
        .iter()
        .filter(|(input, _)| {
            input.contains(
                "[workspace] Dependencies are already installed: `node_modules` was copied in from the user's checkout. Don't reinstall them."
            )
        })
        .collect();
    assert_eq!(told.len(), 1, "{seen:#?}");
    assert!(
        !seen[2].0.contains("Dependencies are already installed"),
        "{}",
        seen[2].0
    );
    // No copy in progress was ever left where git sees it.
    let in_progress = std::fs::read_dir(&worktree)
        .unwrap()
        .flatten()
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".brigadier-warming-")
        });
    assert!(!in_progress);
    // The checkout's own copy is untouched.
    assert!(flow.repo.join("node_modules/left-pad/index.js").is_file());
    flow.stop().await;
}
