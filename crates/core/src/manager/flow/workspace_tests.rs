//! The right sidebar's reads of a checkout: its tree with what it ignores and how each file
//! differs, an ignored folder read on its own and never outside the checkout, and the watch
//! that says when to read again.

use std::path::{Path, PathBuf};
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
        _ => unreachable!(),
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

/// Waits until the watch hears `dir`, then until it is quiet. On a busy Mac a new watch hears
/// nothing for its first seconds, and what is written meanwhile is never told: a file is
/// written until it is.
async fn live(watch: &mut crate::manager::workspace::WorkspaceWatch, dir: &Path) {
    let deadline = std::time::Instant::now() + *super::PATIENCE;
    for round in 0.. {
        std::fs::write(dir.join("watched.txt"), format!("{round}\n")).unwrap();
        if hears(watch, Duration::from_secs(1)).await {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the watch never heard {}",
            dir.display()
        );
    }
    // What it heard may come seconds late on a busy machine, the setup's own writes too.
    while hears(watch, Duration::from_secs(2)).await {}
}

/// The watch hears a file written, created and deleted, and a file staged or a branch moved
/// in a session's own worktree (whose index and refs git keeps outside it), but not a build writing into an ignored
/// folder.
#[ignore = "macOS drops FSEvents under the suite's write storm; run serially, see full-checks"]
#[tokio::test]
async fn the_watch_hears_files_and_staging_but_not_ignored_folders() {
    // A change is said once the watch has read git again after the one before, which takes a
    // while on a busy machine: the watch is waited for, not timed.
    let said = *super::PATIENCE;
    let (flow, target) = flow("workspace-watch").await;
    let repo = flow.repo.clone();
    std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    std::fs::create_dir_all(repo.join("target")).unwrap();
    let mut watch = flow.manager.watch_workspace(&target).await.unwrap();
    live(&mut watch, &repo).await;

    std::fs::write(repo.join("target/build.o"), "object").unwrap();
    assert!(
        !hears(&mut watch, Duration::from_millis(1200)).await,
        "an ignored folder counts"
    );
    std::fs::write(repo.join("README.md"), "# Edited\n").unwrap();
    assert!(hears(&mut watch, said).await, "an edit");
    std::fs::write(repo.join("fresh.txt"), "new\n").unwrap();
    assert!(hears(&mut watch, said).await, "a new file");
    std::fs::remove_file(repo.join("fresh.txt")).unwrap();
    assert!(hears(&mut watch, said).await, "a deletion");
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
    live(&mut watch, &checkout).await;
    git(&checkout, &["add", "notes.txt"]);
    assert!(hears(&mut watch, said).await, "staging");
    // Undoing a commit moves only the branch, kept in the main repository too.
    git(&checkout, &["commit", "-q", "-m", "Notes"]);
    while hears(&mut watch, Duration::from_millis(800)).await {}
    git(&checkout, &["reset", "-q", "--soft", "HEAD~1"]);
    assert!(hears(&mut watch, said).await, "a branch moved");
    // A branch switched to is watched from then on, also in a folder new to git.
    git(&checkout, &["checkout", "-q", "-b", "topic/switched"]);
    assert!(hears(&mut watch, said).await, "a switch");
    git(&checkout, &["commit", "-q", "-m", "Notes"]);
    while hears(&mut watch, Duration::from_millis(800)).await {}
    git(&checkout, &["reset", "-q", "--soft", "HEAD~1"]);
    assert!(
        hears(&mut watch, said).await,
        "the switched-to branch moved"
    );
    drop(watch);
    flow.stop().await;
}

/// A worktree of the project in a scratch folder, on a new branch of its own.
fn user_worktree(flow: &Flow, name: &str) -> PathBuf {
    let path = std::env::temp_dir()
        .join(format!("user-worktree-{}", uuid::Uuid::new_v4()))
        .join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    git(
        &flow.repo,
        &["worktree", "add", "-q", "-b", name, path.to_str().unwrap()],
    );
    path.canonicalize().unwrap()
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

/// The session's own worktree, once it has one.
async fn session_worktree(flow: &Flow) -> PathBuf {
    flow.say("Hi.").await;
    flow.settled().await;
    match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) => PathBuf::from(path),
        other => panic!("a session with a worktree, not {other:?}"),
    }
}

/// A draft's file and Review tabs read each checkout on its own: two worktrees and the project's
/// folder with the same path each give their own, and a session's copy goes through the one
/// resolver and is refused.
#[tokio::test]
async fn a_targets_file_and_review_read_that_checkout_alone() {
    use crate::work::ReviewScope;

    let (flow, project) = flow("target-file-review").await;
    let WorkspaceTarget::Project { project_id } = project.clone() else {
        unreachable!()
    };
    let session = session_worktree(&flow).await;
    let (one, other) = (
        user_worktree(&flow, "tab-one"),
        user_worktree(&flow, "tab-other"),
    );
    let worktree = |path: &Path| WorkspaceTarget::Worktree {
        project_id: project_id.clone(),
        path: text(path),
    };
    let places = [
        (project.clone(), flow.repo.clone(), "folder"),
        (worktree(&one), one.clone(), "one"),
        (worktree(&other), other.clone(), "other"),
    ];
    for (_, checkout, name) in &places {
        std::fs::write(checkout.join("README.md"), format!("# {name}\n")).unwrap();
    }
    std::fs::write(flow.repo.join("only-here.txt"), "folder\n").unwrap();
    git(&other, &["add", "README.md"]);
    let manager = &flow.manager;
    for (target, _, name) in &places {
        let file = manager
            .read_target_file(target, "README.md".into())
            .await
            .unwrap();
        assert_eq!(file.text.as_deref(), Some(format!("# {name}\n").as_str()));
        let review = |scope| manager.target_review_diff(target, scope, true, false);
        let staged = review(ReviewScope::Staged).await.unwrap();
        let unstaged = review(ReviewScope::Unstaged).await.unwrap();
        let (holds, empty) = if *name == "other" {
            (staged, unstaged)
        } else {
            (unstaged, staged)
        };
        assert!(holds.patch.contains(&format!("+# {name}")), "{name}");
        assert!(empty.files.is_empty(), "{name}");
        // Nothing a session's history answers can be asked of a checkout.
        let branch = review(ReviewScope::Branch).await.unwrap_err();
        assert!(branch.to_string().contains("uncommitted"), "{branch}");
    }
    // A file only in the project's folder is missing from the worktree, as a missing file is.
    assert!(matches!(
        manager
            .read_target_file(&worktree(&one), "only-here.txt".into())
            .await,
        Err(crate::Error::NotFound(_))
    ));
    // Leaving the checkout is refused here too.
    assert!(
        manager
            .read_target_file(&project, "../outside".into())
            .await
            .is_err()
    );
    let refused = worktree(&session);
    for err in [
        manager
            .read_target_file(&refused, "README.md".into())
            .await
            .unwrap_err(),
        manager
            .target_review_diff(&refused, ReviewScope::Unstaged, true, false)
            .await
            .unwrap_err(),
    ] {
        assert!(err.to_string().contains("not a worktree you made"), "{err}");
    }
    flow.stop().await;
}

/// The user's worktrees are listed, and one's discard and commit leave every other checkout as
/// it was: the other worktree, and the project's folder.
#[tokio::test]
async fn a_worktree_targets_discard_and_commit_stay_in_that_worktree() {
    use crate::work::{SourceChange, SourceScope};

    let (flow, target) = flow("worktree-target").await;
    let WorkspaceTarget::Project { project_id } = target else {
        unreachable!()
    };
    let session = session_worktree(&flow).await;
    let (one, other) = (
        user_worktree(&flow, "mine-one"),
        user_worktree(&flow, "mine-other"),
    );
    let listed = flow.manager.user_worktrees(&project_id).await.unwrap();
    assert_eq!(listed, [text(&one), text(&other)], "not {session:?}");

    for checkout in [&one, &other, &flow.repo] {
        std::fs::write(checkout.join("README.md"), "# Mine\n").unwrap();
        std::fs::write(checkout.join("new.txt"), "new\n").unwrap();
    }
    let status = |path: &Path| git(path, &["status", "--porcelain"]);
    let (other_before, folder_before) = (status(&other), status(&flow.repo));
    let heads = || {
        (
            git(&other, &["rev-parse", "HEAD"]),
            git(&flow.repo, &["rev-parse", "HEAD"]),
        )
    };
    let heads_before = heads();

    let state = flow
        .manager
        .worktree_source_state(&project_id, &text(&one))
        .await
        .unwrap();
    assert_eq!(state.branch.as_deref(), Some("mine-one"));
    assert_eq!(statuses(&state.changes).len(), 2);

    let state = flow
        .manager
        .change_worktree_source(
            &project_id,
            &text(&one),
            SourceChange::Discard {
                scope: SourceScope::Unstaged,
                paths: Some(vec!["README.md".into()]),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        statuses(&state.changes),
        [("new.txt".to_owned(), SourceStatus::Untracked)]
    );
    assert_eq!(status(&other), other_before, "the other worktree");
    assert_eq!(status(&flow.repo), folder_before, "the project's folder");

    // Nothing staged: Commit's "stage all" takes this checkout's changes, and only its.
    let outcome = flow
        .manager
        .commit_worktree_changes(
            &project_id,
            &text(&one),
            Some("Add new".into()),
            true,
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!(outcome.branch, "mine-one");
    assert_eq!(status(&one), "");
    assert_eq!(git(&one, &["log", "-1", "--format=%s"]), "Add new");
    assert_eq!(status(&other), other_before, "the other worktree");
    assert_eq!(status(&flow.repo), folder_before, "the project's folder");
    assert_eq!(heads(), heads_before);
    flow.stop().await;
}

/// The one resolver refuses, on every call, what isn't a worktree the user made: a session's
/// worktree, a worker's, the project's folder, and a folder that is no worktree of it.
#[tokio::test]
async fn a_worktree_target_refuses_what_brigadier_uses_and_what_is_not_listed() {
    use crate::work::{SourceAction, SourceChange};

    let (flow, target) = flow("worktree-refused").await;
    let WorkspaceTarget::Project { project_id } = target else {
        unreachable!()
    };
    let session = session_worktree(&flow).await;
    std::fs::write(session.join("kept.txt"), "kept\n").unwrap();
    // Where a worker's worktree is: Brigadier's own folder, whichever board names it.
    let sessions_dir = flow.manager.runtime.platform().paths().sessions_dir.clone();
    let worker = sessions_dir.join("worker-check").join("work");
    std::fs::create_dir_all(worker.parent().unwrap()).unwrap();
    git(
        &flow.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "worker-check",
            worker.to_str().unwrap(),
        ],
    );
    let stray = std::env::temp_dir().join(format!("not-a-worktree-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&stray).unwrap();

    let refused = [
        text(&session),
        // The same folder by another way to it: a link resolves to what it leads to.
        format!(
            "{}/../{}",
            session.display(),
            session.file_name().unwrap().display()
        ),
        text(&worker),
        text(&flow.repo),
        text(&stray),
        "relative/path".to_owned(),
    ];
    let manager = &flow.manager;
    assert!(
        manager
            .user_worktrees(&project_id)
            .await
            .unwrap()
            .is_empty()
    );
    for path in refused {
        let refused = |err: crate::Error| {
            assert!(
                err.to_string().contains("not a worktree you made"),
                "{path}: {err}"
            );
        };
        let target = WorkspaceTarget::Worktree {
            project_id: project_id.clone(),
            path: path.clone(),
        };
        refused(
            manager
                .worktree_source_state(&project_id, &path)
                .await
                .unwrap_err(),
        );
        refused(
            manager
                .change_worktree_source(&project_id, &path, SourceChange::Stage { paths: None })
                .await
                .unwrap_err(),
        );
        refused(
            manager
                .commit_worktree_changes(&project_id, &path, Some("No".into()), true, false, false)
                .await
                .unwrap_err(),
        );
        refused(
            manager
                .write_source_message(&target, true)
                .await
                .unwrap_err(),
        );
        refused(
            manager
                .source_action(&target, SourceAction::Fetch)
                .await
                .unwrap_err(),
        );
        refused(manager.source_branches(&target).await.unwrap_err());
        refused(manager.workspace_tree(&target).await.unwrap_err());
        assert!(manager.watch_workspace(&target).await.is_err());
    }
    assert_eq!(
        git(&session, &["status", "--porcelain"]),
        "?? kept.txt",
        "the session's worktree is as it was"
    );
    flow.stop().await;
}

/// Listing worktrees runs on every draft open and worktree read: with nothing outside Brigadier's
/// own folder it reads no board, and with a candidate it reads streams without keeping boards.
#[tokio::test]
async fn listing_worktrees_keeps_no_boards() {
    let (flow, target) = flow("worktree-boards").await;
    let WorkspaceTarget::Project { project_id } = target else {
        unreachable!()
    };
    let session = session_worktree(&flow).await;
    // A conversation nothing has opened since the daemon started: its board isn't loaded.
    let idle = flow
        .manager
        .create_conversation(
            crate::model::ConversationKind::Chat,
            None,
            Some("Idle".into()),
            None,
        )
        .await
        .unwrap()
        .id;
    let kept = flow.core.kept_boards().await;
    assert!(!kept.contains(&idle), "the check needs a board not loaded");

    assert!(
        flow.manager
            .user_worktrees(&project_id)
            .await
            .unwrap()
            .is_empty(),
        "only {session:?}, which is the session's"
    );
    assert_eq!(flow.core.kept_boards().await, kept, "no candidates");

    let mine = user_worktree(&flow, "mine-boards");
    assert_eq!(
        flow.manager.user_worktrees(&project_id).await.unwrap(),
        [text(&mine)]
    );
    assert_eq!(flow.core.kept_boards().await, kept, "a candidate");
    flow.stop().await;
}

/// The next change the set says within a while, if any.
async fn next_target(
    watches: &mut crate::manager::workspace::WorkspaceWatches,
    within: Duration,
) -> Option<WorkspaceTarget> {
    tokio::time::timeout(within, watches.next_change())
        .await
        .ok()
}

/// A connection watches a set of targets: each change names its target, a request replaces the
/// whole set, one that can't be watched leaves the others running, and a removed one says nothing.
#[tokio::test]
async fn the_watch_set_names_its_targets_and_is_replaced_whole() {
    let (flow, folder) = flow("watch-set").await;
    let project_id = match &folder {
        WorkspaceTarget::Project { project_id } => project_id.clone(),
        WorkspaceTarget::Conversation { .. } | WorkspaceTarget::Worktree { .. } => unreachable!(),
    };
    let path = user_worktree(&flow, "watched");
    let worktree = WorkspaceTarget::Worktree {
        project_id: project_id.clone(),
        path: text(&path),
    };
    let gone = WorkspaceTarget::Worktree {
        project_id: project_id.clone(),
        path: "/not/a/worktree".to_owned(),
    };
    let said = Duration::from_secs(10);
    let quiet = Duration::from_millis(800);
    let mut watches = crate::manager::workspace::WorkspaceWatches::default();

    let watching = flow
        .manager
        .watch_workspaces(
            &mut watches,
            vec![folder.clone(), gone.clone(), worktree.clone()],
        )
        .await;
    assert_eq!(
        watching,
        [folder.clone(), worktree.clone()],
        "the failed one alone is left out"
    );
    // The watchers take a moment to start; their first events may be of the start.
    tokio::time::sleep(Duration::from_millis(500)).await;
    while next_target(&mut watches, quiet).await.is_some() {}

    std::fs::write(path.join("in-worktree.txt"), "x\n").unwrap();
    assert_eq!(
        next_target(&mut watches, said).await,
        Some(worktree.clone())
    );
    while next_target(&mut watches, quiet).await.is_some() {}
    std::fs::write(flow.repo.join("in-folder.txt"), "x\n").unwrap();
    assert_eq!(next_target(&mut watches, said).await, Some(folder.clone()));
    while next_target(&mut watches, quiet).await.is_some() {}

    // The worktree's row went: only the folder is said from now on.
    let watching = flow
        .manager
        .watch_workspaces(&mut watches, vec![folder.clone()])
        .await;
    assert_eq!(watching, std::slice::from_ref(&folder));
    tokio::time::sleep(Duration::from_millis(500)).await;
    while next_target(&mut watches, quiet).await.is_some() {}
    std::fs::write(path.join("in-worktree-again.txt"), "x\n").unwrap();
    assert_eq!(next_target(&mut watches, quiet).await, None);

    let watching = flow
        .manager
        .watch_workspaces(&mut watches, Vec::new())
        .await;
    assert!(watching.is_empty());
    std::fs::write(flow.repo.join("in-folder-again.txt"), "x\n").unwrap();
    assert_eq!(
        next_target(&mut watches, quiet).await,
        None,
        "none stops watching"
    );
    flow.stop().await;
}

/// The Source panel's Sync Changes counts both ways, against the upstream as last fetched.
#[tokio::test]
async fn source_state_counts_commits_to_pull_and_to_push() {
    let (flow, target) = flow("source-behind").await;
    let WorkspaceTarget::Project { project_id } = target else {
        unreachable!()
    };
    let repo = &flow.repo;
    let origin = std::env::temp_dir().join(format!("source-behind-{}", uuid::Uuid::new_v4()));
    let init = crate::test_git_command()
        .args(["init", "-q", "--bare", origin.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init.status.success(), "{init:?}");
    let branch = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]);
    git(repo, &["remote", "add", "origin", origin.to_str().unwrap()]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "Theirs"]);
    git(repo, &["push", "-q", "-u", "origin", &branch]);
    git(repo, &["reset", "-q", "--hard", "HEAD~1"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "Mine one"]);
    git(repo, &["commit", "-q", "--allow-empty", "-m", "Mine two"]);
    let state = flow
        .manager
        .project_source_state(&project_id)
        .await
        .unwrap();
    assert_eq!((state.ahead, state.behind), (2, 1));
    flow.stop().await;
}

/// Unreadable ownership must not turn a listed checkout into a user-owned one.
#[tokio::test]
async fn worktree_ownership_fails_closed_on_an_unreadable_stream() {
    let (flow, target) = flow("unreadable-ownership").await;
    let WorkspaceTarget::Project { project_id } = target else {
        unreachable!()
    };
    let mine = user_worktree(&flow, "unreadable-mine");
    flow.core
        .store()
        .append(super::seed_events(&format!(
            r#"{{"stream":"{}","kind":"task.updated","payload":{{}}}}"#,
            crate::model::streams::conversation(&flow.conversation),
        )))
        .await
        .unwrap();
    assert!(flow.core.tasks_unkept(&flow.conversation).await.is_err());
    let result = flow.manager.user_worktrees(&project_id).await;
    assert!(result.is_err(), "ownership was unreadable, got {result:?}");
    assert!(
        flow.manager
            .worktree_source_state(&project_id, &text(&mine))
            .await
            .is_err()
    );
    flow.stop().await;
}

#[tokio::test]
async fn worktree_ownership_checks_external_workers_and_symlink_reads() {
    use crate::work::{ReviewScope, SourceChange};
    let (flow, project) = flow("external-ownership").await;
    let WorkspaceTarget::Project { project_id } = project.clone() else {
        unreachable!()
    };
    let worker = user_worktree(&flow, "external-worker");
    let mine = user_worktree(&flow, "external-mine");
    let event = serde_json::json!({
        "stream": crate::model::streams::conversation(&flow.conversation),
        "kind": "task.updated",
        "payload": { "type": "taskUpdated", "task": {
            "id": "external", "conversationId": flow.conversation.0, "number": 1, "position": 0,
            "title": "External worker", "kind": "implement", "spec": "Work",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "done", "attachments": [], "createdAtMs": 0, "updatedAtMs": 0,
            "workspace": { "worktree": text(&worker), "branch": "external-worker", "base": null, "target": "main", "scratch": text(&mine) }
        }}
    });
    flow.core
        .store()
        .append(super::seed_events(&event.to_string()))
        .await
        .unwrap();
    assert_eq!(
        flow.manager.user_worktrees(&project_id).await.unwrap(),
        [text(&mine)]
    );
    let refused = WorkspaceTarget::Worktree {
        project_id: project_id.clone(),
        path: text(&worker),
    };
    assert!(
        flow.manager
            .read_target_file(&refused, "README.md".into())
            .await
            .is_err()
    );
    assert!(
        flow.manager
            .target_review_diff(&refused, ReviewScope::Unstaged, true, false)
            .await
            .is_err()
    );
    assert!(
        flow.manager
            .change_worktree_source(
                &project_id,
                &text(&worker),
                SourceChange::Stage { paths: None }
            )
            .await
            .is_err()
    );
    assert!(
        flow.manager
            .commit_worktree_changes(
                &project_id,
                &text(&worker),
                Some("No".into()),
                true,
                false,
                false
            )
            .await
            .is_err()
    );
    #[cfg(unix)]
    {
        let outside = mine.join("outside-secret.txt");
        std::fs::write(&outside, "PRIVATE CONTENT SENTINEL").unwrap();
        std::os::unix::fs::symlink(&outside, flow.repo.join("escape")).unwrap();
        std::os::unix::fs::symlink(&flow.repo, mine.join("alias")).unwrap();
        assert!(
            flow.manager
                .read_target_file(&project, "escape".into())
                .await
                .is_err()
        );
        assert!(
            flow.manager
                .read_target_file(&project, text(&outside))
                .await
                .is_err()
        );
        assert!(
            flow.manager
                .read_target_file(&project, "../outside-secret.txt".into())
                .await
                .is_err()
        );
        let review = flow
            .manager
            .target_review_diff(&project, ReviewScope::Unstaged, true, false)
            .await
            .unwrap();
        assert!(!review.patch.contains("PRIVATE CONTENT SENTINEL"));
        // A symlink to another checkout is still that checkout, not a way to name a user worktree.
        let alias = WorkspaceTarget::Worktree {
            project_id,
            path: text(&mine.join("alias")),
        };
        assert!(
            flow.manager
                .read_target_file(&alias, "README.md".into())
                .await
                .is_err()
        );
    }
    flow.stop().await;
}
