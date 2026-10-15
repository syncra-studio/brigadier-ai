//! The session's own worktree starts warm (BUILD-PLAN C7): the thread works in it as a worker
//! does in its own, so it gets the same copies of the checkout's dependency installs, and
//! hears so, instead of failing its first check and reinstalling them.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

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

#[tokio::test]
async fn the_threads_worktree_has_the_checkouts_dependencies_and_the_thread_is_told() {
    let seen: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let script: Script = {
        let seen = seen.clone();
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                let installed = turn
                    .add_dirs
                    .iter()
                    .chain([&turn.cwd])
                    .any(|dir| dir.join("node_modules/left-pad/index.js").is_file());
                seen.lock().unwrap().push((turn.input.clone(), installed));
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
    age(&flow.repo.join("node_modules"));
    flow.say("Run the tests").await;
    flow.settled().await;
    {
        let seen = seen.lock().unwrap();
        let (input, installed) = seen.first().expect("the thread took a turn");
        assert!(installed, "the session's worktree has node_modules");
        assert!(
            input.contains(
                "[workspace] Dependencies are already installed: `node_modules` was copied in from the user's checkout. Don't reinstall them."
            ),
            "{input}"
        );
    }
    // Only once: the next message's worktree is the same one.
    flow.say("And again").await;
    flow.settled().await;
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2);
    assert!(
        !seen[1].0.contains("Dependencies are already installed"),
        "{}",
        seen[1].0
    );
    // The checkout's own copy is untouched.
    assert!(flow.repo.join("node_modules/left-pad/index.js").is_file());
    flow.stop().await;
}
