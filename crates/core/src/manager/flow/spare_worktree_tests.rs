//! No spare worktrees (BUILD-PLAN A8): a worker's worktree is made when its task starts, never
//! ahead of a task that may not come, and the spare `task-next-*` worktrees earlier versions left
//! go at the next launch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_providers::Artifact;

use super::{Flow, Options, Reply, Script, git};
use crate::model::TaskId;

/// A thread that answers a question in words and, asked to edit, writes a file in its own
/// workspace; it never delegates.
fn self_working() -> Script {
    Arc::new(|turn| {
        Box::pin(async move {
            if turn.input.contains("Please add a note") {
                turn.write("note.txt", "a note\n");
                return Reply::text("Added note.txt.");
            }
            Reply::text("Done.")
        })
    })
}

/// Every `task-next-*` folder under `dir`, two levels down (the data folder's
/// `worktrees/<project>/`).
fn spare_folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(projects) = std::fs::read_dir(dir.join("worktrees")) else {
        return Vec::new();
    };
    projects
        .flatten()
        .filter_map(|project| std::fs::read_dir(project.path()).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("task-next-"))
        })
        .collect()
}

/// Nothing spare on disk, in the ledger or in the repository's worktree list.
fn no_spares(flow: &Flow) {
    let data = &flow.manager.data_dir;
    assert!(spare_folders(data).is_empty(), "{:?}", spare_folders(data));
    let owners = flow.manager.runtime.ledger().owners();
    assert!(
        !owners
            .iter()
            .any(|(owner, _, _)| owner.starts_with("task:")),
        "{owners:?}"
    );
    let listed = git(&flow.repo, &["worktree", "list"]);
    assert!(!listed.contains("task-next-"), "{listed}");
}

#[tokio::test]
async fn a_question_a_self_edit_and_a_quit_leave_no_spare_worktree() {
    let mut flow = Flow::start("spare-none", Options::default(), self_working()).await;
    flow.say("What does this repository hold?").await;
    flow.settled().await;
    no_spares(&flow);
    flow.say("Please add a note").await;
    let board = flow.settled().await;
    assert!(board.tasks.is_empty(), "the thread worked itself");
    no_spares(&flow);
    flow.restart().await;
    flow.settled().await;
    no_spares(&flow);
    flow.stop().await;
}

#[tokio::test]
async fn the_launch_removes_a_spare_worktree_an_earlier_version_left() {
    let mut flow = Flow::start("spare-orphan", Options::default(), self_working()).await;
    flow.say("Hello").await;
    flow.settled().await;
    // What an earlier version's pre-warm left: a worktree under a task id no task ever took.
    let id = TaskId::generate();
    let worktree = flow
        .manager
        .data_dir
        .join("worktrees")
        .join("p")
        .join(format!("task-next-{}", &id.0[id.0.len() - 8..]));
    std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
    let owner = format!("task:{id}");
    flow.manager
        .runtime
        .ledger()
        .record(
            &owner,
            Artifact::Worktree {
                repo: flow.repo.display().to_string(),
                path: worktree.display().to_string(),
            },
        )
        .await
        .unwrap();
    git(
        &flow.repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            worktree.to_str().unwrap(),
        ],
    );
    assert!(worktree.join("README.md").exists());
    flow.restart().await;
    super::eventually("the spare worktree to go", || {
        flow.manager.runtime.ledger().artifacts(&owner).is_empty() && !worktree.exists()
    })
    .await;
    no_spares(&flow);
    flow.stop().await;
}
