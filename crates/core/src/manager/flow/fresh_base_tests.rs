//! A new tab's branch starts from what the remote has: when someone else pushed to `main` and
//! the local `main` is only behind, the tab's worktree is made from `origin/main` and the
//! thread hears so once. The user's own `main` is left as it is.

use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, git};

#[tokio::test]
async fn a_new_tab_starts_from_origins_newer_main_and_hears_so() {
    let seen: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let script: Script = {
        let seen = seen.clone();
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                let has = turn
                    .add_dirs
                    .iter()
                    .chain([&turn.cwd])
                    .any(|dir| dir.join("UPSTREAM.md").is_file());
                seen.lock().unwrap().push((turn.input.clone(), has));
                Reply::text("Done.")
            })
        })
    };
    let flow = Flow::start("fresh-base", Options::default(), script).await;
    let origin = flow.repo.with_file_name("fresh-base-origin.git");
    git(
        &flow.repo,
        &["clone", "-q", "--bare", ".", origin.to_str().unwrap()],
    );
    git(
        &flow.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&flow.repo, &["fetch", "-q", "origin"]);
    git(
        &flow.repo,
        &["branch", "--set-upstream-to=origin/main", "main"],
    );
    let local = git(&flow.repo, &["rev-parse", "main"]);
    // Someone else pushes to origin's main; this checkout hasn't fetched it.
    let other = flow.repo.with_file_name("fresh-base-other");
    git(
        &flow.repo,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join("UPSTREAM.md"), "upstream\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "Upstream work"]);
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    let pushed = git(&other, &["rev-parse", "HEAD"]);

    flow.say("Look at the code").await;
    flow.settled().await;
    flow.say("And again").await;
    flow.settled().await;
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let (input, has) = &seen[0];
    assert!(has, "the tab's worktree has what origin's main has");
    let line = format!(
        "[git] Your branch starts from `origin/main` at {}, the newest `main`: the local `main` is 1 commit behind it.",
        &pushed[..10]
    );
    assert!(input.contains(&line), "{input}");
    // Said once: the next message's worktree is the same one.
    assert!(
        !seen[1].0.contains("[git] Your branch starts"),
        "{}",
        seen[1].0
    );
    // The user's own main stays where it was.
    assert_eq!(git(&flow.repo, &["rev-parse", "main"]), local);
    flow.stop().await;
}
