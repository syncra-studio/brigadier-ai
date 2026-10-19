//! The right sidebar's reads of a checkout: its tree with what it ignores and how each file
//! differs, an ignored folder read on its own and never outside the checkout, and the watch
//! that says when to read again.

use std::path::PathBuf;
use std::time::Duration;

use super::{Flow, Options, Reply, git};
use crate::model::{Environment, ProjectId, Setup};
use crate::work::{SourceFile, SourceStatus, WorkspaceTarget};

async fn flow(name: &str) -> (Flow, WorkspaceTarget) {
    let flow = Flow::start(
        name,
        Options::default(),
        std::sync::Arc::new(|_| Box::pin(async { Reply::text("Hello.") }) as _),
    )
    .await;
    let project_id = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .expect("a session has a project");
    (flow, WorkspaceTarget::Project { project_id })
}

fn statuses(files: &[SourceFile]) -> Vec<(String, SourceStatus)> {
    files
        .iter()
        .map(|file| (file.path.clone(), file.status))
        .collect()
}

/// The tree lists tracked and untracked files, a deleted one still (git still tracks it), and
/// what is ignored: a folder once, a file by name. Each file's status says how it differs.
#[tokio::test]
async fn the_tree_lists_ignored_entries_and_every_kind_of_change() {
    let (flow, target) = flow("workspace-tree").await;
    let repo = &flow.repo;
    std::fs::write(repo.join(".gitignore"), "target/\n*.log\n").unwrap();
    std::fs::write(repo.join("gone.txt"), "gone\n").unwrap();
    std::fs::write(repo.join("old.txt"), "old\n").unwrap();
    std::fs::write(repo.join("both.txt"), "base\n").unwrap();
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-m", "Files"]);
    // A conflict first: both sides changed the same line.
    git(repo, &["checkout", "-q", "-b", "side"]);
    std::fs::write(repo.join("both.txt"), "side\n").unwrap();
    git(repo, &["commit", "-qam", "Side"]);
    git(repo, &["checkout", "-q", "-"]);
    std::fs::write(repo.join("both.txt"), "main\n").unwrap();
    git(repo, &["commit", "-qam", "Main"]);
    let merged = crate::test_git_command()
        .args(["merge", "side"])
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(!merged.status.success(), "the merge conflicts");
    std::fs::create_dir_all(repo.join("target/debug")).unwrap();
    std::fs::write(repo.join("target/debug/app"), "bin").unwrap();
    std::fs::write(repo.join("build.log"), "log").unwrap();
    std::fs::write(repo.join("README.md"), "# Changed\n").unwrap();
    std::fs::write(repo.join("new.txt"), "new\n").unwrap();
    std::fs::write(repo.join("added.txt"), "added\n").unwrap();
    git(repo, &["add", "added.txt"]);
    std::fs::remove_file(repo.join("gone.txt")).unwrap();
    git(repo, &["mv", "old.txt", "moved.txt"]);

    let tree = flow.manager.workspace_tree(&target).await.unwrap();
    for file in [
        "README.md",
        "new.txt",
        "added.txt",
        "gone.txt",
        "moved.txt",
        "both.txt",
    ] {
        assert!(
            tree.files.contains(&file.to_owned()),
            "{file}: {:?}",
            tree.files
        );
    }
    assert!(!tree.files.iter().any(|file| file.starts_with("target")));
    assert_eq!(tree.ignored, ["build.log", "target/"]);
    assert!(!tree.truncated && !tree.ignored_truncated);

    let project_id = match &target {
        WorkspaceTarget::Project { project_id } => project_id.clone(),
        WorkspaceTarget::Conversation { .. } => unreachable!(),
    };
    let state = flow
        .manager
        .project_source_state(&project_id)
        .await
        .unwrap();
    let branch = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(
        state.branch.as_deref(),
        Some(branch.trim()),
        "the branch checked out"
    );
    let staged = statuses(&state.staged);
    let changes = statuses(&state.changes);
    assert!(
        staged.contains(&("added.txt".into(), SourceStatus::Added)),
        "{staged:?}"
    );
    assert!(
        staged.contains(&("moved.txt".into(), SourceStatus::Renamed)),
        "{staged:?}"
    );
    for expected in [
        ("README.md", SourceStatus::Modified),
        ("new.txt", SourceStatus::Untracked),
        ("gone.txt", SourceStatus::Deleted),
        ("both.txt", SourceStatus::Conflicted),
    ] {
        assert!(
            changes.contains(&(expected.0.into(), expected.1)),
            "{expected:?}: {changes:?}"
        );
    }
    flow.stop().await;
}

/// An ignored folder opened in the tree is read from disk, folders first. Nothing outside the
/// checkout is read: not `..`, an absolute path, `.git`, a link leading out, a file, nor a
/// project that doesn't exist.
#[tokio::test]
async fn a_folder_is_read_only_inside_the_checkout() {
    let (flow, target) = flow("workspace-folder").await;
    let repo = &flow.repo;
    std::fs::create_dir_all(repo.join("target/debug")).unwrap();
    std::fs::write(repo.join("target/out.txt"), "out").unwrap();
    let outside = repo.parent().unwrap().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "keep").unwrap();
    std::os::unix::fs::symlink(&outside, repo.join("target/escape")).unwrap();
    std::os::unix::fs::symlink(repo.join(".git"), repo.join("git-alias")).unwrap();

    let folder = flow
        .manager
        .workspace_folder(&target, "target".into())
        .await
        .unwrap();
    let entries: Vec<(String, bool)> = folder
        .entries
        .iter()
        .map(|entry| (entry.name.clone(), entry.folder))
        .collect();
    assert_eq!(
        entries,
        [
            ("debug".into(), true),
            ("escape".into(), true),
            ("out.txt".into(), false)
        ]
    );
    for path in [
        "",
        "..",
        "../outside",
        "/etc",
        ".git",
        ".git/refs",
        "target/../..",
        "./target",
        "target/escape",
        "git-alias",
        "target/out.txt",
        "missing",
    ] {
        let refused = flow.manager.workspace_folder(&target, path.into()).await;
        assert!(refused.is_err(), "{path} was read: {refused:?}");
    }
    let missing = WorkspaceTarget::Project {
        project_id: ProjectId("no-such-project".into()),
    };
    assert!(flow.manager.workspace_tree(&missing).await.is_err());
    assert!(
        flow.manager
            .workspace_folder(&missing, "target".into())
            .await
            .is_err()
    );
    assert!(flow.manager.watch_workspace(&missing).await.is_err());
    flow.stop().await;
}

/// Waits for the watch to say the checkout changed; false when it doesn't within `wait`.
async fn hears(watch: &mut crate::manager::workspace::WorkspaceWatch, wait: Duration) -> bool {
    tokio::time::timeout(wait, watch.changed()).await.is_ok()
}

/// The watch hears a file written, created and deleted, and a file staged or a branch moved
/// in a session's own worktree (whose index and refs git keeps outside it), but not a build writing into an ignored
/// folder.
#[tokio::test]
async fn the_watch_hears_files_and_staging_but_not_ignored_folders() {
    let (flow, target) = flow("workspace-watch").await;
    let repo = flow.repo.clone();
    std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(repo.join("target")).unwrap();
    let mut watch = flow.manager.watch_workspace(&target).await.unwrap();
    // What the setup itself did may still arrive.
    while hears(&mut watch, Duration::from_millis(800)).await {}

    std::fs::write(repo.join("target/build.o"), "object").unwrap();
    assert!(
        !hears(&mut watch, Duration::from_millis(1200)).await,
        "an ignored folder counts"
    );
    std::fs::write(repo.join("README.md"), "# Edited\n").unwrap();
    assert!(hears(&mut watch, Duration::from_secs(3)).await, "an edit");
    std::fs::write(repo.join("fresh.txt"), "new\n").unwrap();
    assert!(
        hears(&mut watch, Duration::from_secs(3)).await,
        "a new file"
    );
    std::fs::remove_file(repo.join("fresh.txt")).unwrap();
    assert!(
        hears(&mut watch, Duration::from_secs(3)).await,
        "a deletion"
    );
    drop(watch);

    // A session's own worktree: staging only touches its index, kept in the main repository.
    flow.say("Hi.").await;
    flow.settled().await;
    let checkout = match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) => PathBuf::from(path),
        other => panic!("a session with a worktree, not {other:?}"),
    };
    std::fs::write(checkout.join("notes.txt"), "notes\n").unwrap();
    let session = WorkspaceTarget::Conversation {
        conversation_id: flow.conversation.clone(),
    };
    let mut watch = flow.manager.watch_workspace(&session).await.unwrap();
    while hears(&mut watch, Duration::from_millis(800)).await {}
    git(&checkout, &["add", "notes.txt"]);
    assert!(hears(&mut watch, Duration::from_secs(3)).await, "staging");
    // Undoing a commit moves only the branch, kept in the main repository too.
    git(&checkout, &["commit", "-q", "-m", "Notes"]);
    while hears(&mut watch, Duration::from_millis(800)).await {}
    git(&checkout, &["reset", "-q", "--soft", "HEAD~1"]);
    assert!(
        hears(&mut watch, Duration::from_secs(3)).await,
        "a branch moved"
    );
    // A branch switched to is watched from then on, also in a folder new to git.
    git(&checkout, &["checkout", "-q", "-b", "topic/switched"]);
    assert!(hears(&mut watch, Duration::from_secs(3)).await, "a switch");
    git(&checkout, &["commit", "-q", "-m", "Notes"]);
    while hears(&mut watch, Duration::from_millis(800)).await {}
    git(&checkout, &["reset", "-q", "--soft", "HEAD~1"]);
    assert!(
        hears(&mut watch, Duration::from_secs(3)).await,
        "the switched-to branch moved"
    );
    drop(watch);
    flow.stop().await;
}

/// A single change is said at once, not after a burst's settling time; a burst of writes (a
/// build's, a checkout's) is said a few times while it goes on, the last after its end.
#[tokio::test]
async fn the_watch_says_a_change_at_once_and_a_burst_a_few_times() {
    let (flow, target) = flow("workspace-watch-quick").await;
    let repo = flow.repo.clone();
    let mut watch = flow.manager.watch_workspace(&target).await.unwrap();
    while hears(&mut watch, Duration::from_millis(800)).await {}

    let mut quickest = Duration::MAX;
    for round in 0..3 {
        let wrote = std::time::Instant::now();
        std::fs::write(repo.join(format!("quick-{round}.txt")), "new\n").unwrap();
        assert!(hears(&mut watch, Duration::from_secs(3)).await, "a write");
        quickest = quickest.min(wrote.elapsed());
        while hears(&mut watch, Duration::from_millis(600)).await {}
    }
    // The old watch waited 200 ms after a change's first event before saying it.
    assert!(quickest < Duration::from_millis(150), "{quickest:?}");

    std::fs::create_dir_all(repo.join("generated")).unwrap();
    while hears(&mut watch, Duration::from_millis(600)).await {}
    let (tx, mut said) = tokio::sync::mpsc::unbounded_channel();
    let listening = tokio::spawn(async move {
        while watch.changed().await.is_some() {
            if tx.send(std::time::Instant::now()).is_err() {
                return;
            }
        }
    });
    let started = std::time::Instant::now();
    let mut writes = 0;
    // When the last write began: on a loaded machine the sleep after it can outlast its say.
    let mut last_write = started;
    while started.elapsed() < Duration::from_millis(1500) {
        last_write = std::time::Instant::now();
        std::fs::write(repo.join(format!("generated/out-{writes}.txt")), "out\n").unwrap();
        writes += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    listening.abort();
    let mut times = Vec::new();
    while let Ok(at) = said.try_recv() {
        times.push(at);
    }
    assert!(
        (2..=10).contains(&times.len()),
        "{} writes said {} times",
        writes,
        times.len()
    );
    assert!(
        times.last().is_some_and(|last| *last >= last_write),
        "said after the burst's last write"
    );
    flow.stop().await;
}
