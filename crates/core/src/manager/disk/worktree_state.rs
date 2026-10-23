//! Cleanup advice uses the recorded base, including equivalent squash patches.
use std::io::Write;
use std::process::{Command, Stdio};

use brigadier_git::Repo;

/// Ancestry, cherry-picks, or a squash commit containing the branch's aggregate patch.
/// Errors are conservative: without evidence of a merge the worktree stays.
pub(crate) fn merged_into(repo: &Repo, branch: &str, base: &str) -> bool {
    if repo.is_merged(branch, base).unwrap_or(false) {
        return true;
    }
    let Ok(Some(head)) = repo.branch_tip(branch) else {
        return false;
    };
    let Ok(Some(target)) = repo.branch_tip(base) else {
        return false;
    };
    let Ok(fork) = repo.merge_base(&head, &target) else {
        return false;
    };
    let Ok(cherry) = repo.read_only(&["cherry", &target.0, &head.0]) else {
        return false;
    };
    let range = format!("{}..{}", fork.0, head.0);
    let no_merges = repo
        .read_only(&["rev-list", "--merges", &range])
        .is_ok_and(|merges| merges.trim().is_empty());
    // `cherry` omits merge commits, including unique conflict resolutions.
    if no_merges && !cherry.is_empty() && cherry.lines().all(|line| line.starts_with("- ")) {
        return true;
    }
    let Ok(diff) = repo.diff(&fork, &head) else {
        return false;
    };
    let Some(patch) = patch_ids(&diff).and_then(|ids| ids.into_iter().next()) else {
        return false;
    };
    let range = format!("{}..{}", fork.0, target.0);
    let Ok(history) = repo.read_only(&[
        "log",
        "--no-ext-diff",
        "--no-merges",
        "--format=commit %H",
        "-p",
        &range,
    ]) else {
        return false;
    };
    patch_ids(&history).is_some_and(|ids| ids.contains(&patch))
}

fn patch_ids(patch: &str) -> Option<Vec<String>> {
    let mut child = Command::new("git")
        .args(["patch-id", "--stable"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Write and read concurrently: history can exceed either pipe's capacity.
    let mut stdin = child.stdin.take()?;
    let patch = patch.as_bytes().to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&patch));
    let output = child.wait_with_output().ok();
    writer.join().ok()?.ok()?;
    let output = output.filter(|out| out.status.success())?;
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split_whitespace().next().map(str::to_owned))
            .collect(),
    )
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Advice {
    Merged,
    Empty,
    Idle,
    NeedsAsk,
}

pub(super) fn advice(
    repo: &Repo,
    branch: &str,
    base: &str,
    dirty: bool,
    idle: bool,
) -> Option<Advice> {
    if dirty {
        return Some(Advice::NeedsAsk);
    }
    let head = repo.branch_tip(branch).ok()??;
    let target = repo.branch_tip(base).ok()??;
    if head == target {
        return Some(Advice::Empty);
    }
    if merged_into(repo, branch, base) {
        return Some(Advice::Merged);
    }
    if repo.count_commits(&target, &head).ok()? == 0 {
        return Some(Advice::Empty);
    }
    idle.then_some(Advice::Idle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn git(path: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn commit(path: &Path, file: &str) {
        std::fs::write(path.join(file), file).unwrap();
        git(path, &["add", file]);
        git(path, &["commit", "-m", file]);
    }
    #[test]
    fn cleanup_recognises_merged_squashed_empty_idle_and_dirty_worktrees() {
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "20261022-0805-take-over-5-sessions-w81-cleanup-{}",
            uuid::Uuid::new_v4()
        )));
        let root = &fixture.0;
        std::fs::create_dir_all(root).unwrap();
        git(root, &["init", "-b", "chosen-base"]);
        git(root, &["config", "user.email", "test@example.test"]);
        git(root, &["config", "user.name", "Test"]);
        commit(root, "initial");
        let make = |name: &str| {
            let path = root.join(name);
            git(
                root,
                &[
                    "worktree",
                    "add",
                    "-b",
                    name,
                    path.to_str().unwrap(),
                    "chosen-base",
                ],
            );
            path
        };
        let merged = make("merged");
        commit(&merged, "merged-file");
        git(root, &["merge", "--no-ff", "merged", "-m", "merge"]);
        let squash = make("squashed");
        commit(&squash, "squash-one");
        commit(&squash, "squash-two");
        git(root, &["merge", "--squash", "squashed"]);
        git(root, &["commit", "-m", "squash"]);
        let empty = make("empty");
        let idle = make("idle");
        commit(&idle, "idle-file");
        let engine = brigadier_git::Git::new("git".into(), std::env::vars_os().collect());
        let check = |path: &Path, branch: &str, idle| {
            let repo = engine.open(path).unwrap();
            let dirty = !repo.state().unwrap().dirty_files.is_empty();
            advice(&repo, branch, "chosen-base", dirty, idle)
        };
        assert_eq!(check(&merged, "merged", false), Some(Advice::Merged));
        assert_eq!(check(&squash, "squashed", false), Some(Advice::Merged));
        assert_eq!(check(&empty, "empty", false), Some(Advice::Empty));
        assert_eq!(check(&idle, "idle", true), Some(Advice::Idle));
        assert_eq!(check(&idle, "idle", false), None);
        std::fs::write(merged.join("unsaved"), "keep me").unwrap();
        assert_eq!(check(&merged, "merged", true), Some(Advice::NeedsAsk));
        assert!(merged.join("unsaved").is_file());
        assert!(!merged_into(
            &engine.open(&idle).unwrap(),
            "idle",
            "missing-base"
        ));
    }
    #[test]
    fn cherry_picked_commits_do_not_hide_unique_merge_resolutions() {
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "20261022-0805-take-over-5-sessions-w81-merge-resolution-{}",
            uuid::Uuid::new_v4()
        )));
        let root = &fixture.0;
        std::fs::create_dir_all(root).unwrap();
        git(root, &["init", "-b", "chosen-base"]);
        git(root, &["config", "user.email", "test@example.test"]);
        git(root, &["config", "user.name", "Test"]);
        commit(root, "initial");
        git(root, &["switch", "-c", "feature"]);
        commit(root, "feature-file");
        git(root, &["switch", "-c", "side", "chosen-base"]);
        commit(root, "side-file");
        git(root, &["switch", "feature"]);
        git(root, &["merge", "--no-ff", "--no-commit", "side"]);
        commit(root, "unique-resolution");
        git(root, &["switch", "chosen-base"]);
        commit(root, "upstream");
        git(root, &["cherry-pick", "feature^1", "side"]);
        let engine = brigadier_git::Git::new("git".into(), std::env::vars_os().collect());
        let repo = engine.open(root).unwrap();
        let cherry = repo
            .read_only(&["cherry", "chosen-base", "feature"])
            .unwrap();
        assert!(
            cherry.lines().all(|line| line.starts_with("- ")),
            "{cherry}"
        );
        assert!(!merged_into(&repo, "feature", "chosen-base"));
    }
}
