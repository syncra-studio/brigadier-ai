//! What git shows of a worker's branch against the branch it merges into (STRIP-DOWN-PLAN
//! §9.1 O2, P1 of the harness research, D1): read on demand, never asked of the worker.

use crate::{CommitInfo, Oid, Repo, Result, repo::TreeMerge};

/// Where a branch stands against the branch it merges into, as `git merge-tree` and the commit
/// graph show it (O2's states, plus `Nothing`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeStanding {
    /// No commit of its own: its tip is where it started.
    Nothing,
    /// It has commits, and every one is in the base.
    Merged,
    /// Its commits would merge cleanly, and the base has not moved since it forked: a
    /// fast-forward.
    Clean,
    /// Its commits would merge cleanly, but the base has commits it lacks.
    Behind,
    /// `git merge-tree` reports conflicts.
    Conflicts,
}

/// A branch's facts against its base.
#[derive(Debug, Clone, PartialEq)]
pub struct BranchFacts {
    /// The branch's tip.
    pub tip: Oid,
    /// Its commits not in the base, newest first, at most the limit asked for.
    pub commits: Vec<CommitInfo>,
    /// How many commits it has that the base lacks.
    pub ahead: u32,
    /// How many commits the base has that it lacks.
    pub behind: u32,
    /// Where it stands against the base.
    pub merge: MergeStanding,
    /// With [`MergeStanding::Conflicts`]: the conflicted paths.
    pub conflicts: Vec<String>,
}

impl Repo {
    /// Local branch `branch`'s facts against local branch `into`; `start`, the commit it
    /// started from when recorded, tells a branch with no commits from one fully merged.
    pub fn branch_facts(
        &self,
        branch: &str,
        into: &str,
        start: Option<&Oid>,
        limit: usize,
    ) -> Result<BranchFacts> {
        let tip = self.branch_commit(branch)?;
        let base = self.branch_commit(into)?;
        let ahead = self.count_commits(&base, &tip)?;
        let behind = self.count_commits(&tip, &base)?;
        let commits = if ahead == 0 {
            Vec::new()
        } else {
            self.log_range(&base, &tip, limit)?
        };
        let mut conflicts = Vec::new();
        let merge = if ahead == 0 {
            if start.is_some_and(|start| *start == tip) {
                MergeStanding::Nothing
            } else {
                MergeStanding::Merged
            }
        } else if behind == 0 {
            MergeStanding::Clean
        } else {
            match self.merge_tree(&base, &tip)? {
                TreeMerge::Ready(_) => MergeStanding::Behind,
                TreeMerge::Conflicts(paths) => {
                    conflicts = paths;
                    MergeStanding::Conflicts
                }
            }
        };
        Ok(BranchFacts {
            tip,
            commits,
            ahead,
            behind,
            merge,
            conflicts,
        })
    }

    /// The uncommitted files of the checkout local branch `branch` is checked out in (this
    /// repository's or one of its worktrees); `None` when no checkout has it.
    pub fn checkout_of_dirty(&self, branch: &str) -> Result<Option<Vec<String>>> {
        let Some(path) = self
            .worktrees()?
            .into_iter()
            .find(|worktree| worktree.branch.as_deref() == Some(branch))
            .map(|worktree| worktree.path)
        else {
            return Ok(None);
        };
        Ok(Some(self.git.open(&path)?.dirty_files()?))
    }

    /// This checkout's staged, unstaged and untracked files, ignored ones left out.
    pub fn dirty_files(&self) -> Result<Vec<String>> {
        Ok(self.status(false)?.dirty())
    }

    /// The commits of `to` that `from` lacks, newest first, at most `limit`.
    fn log_range(&self, from: &Oid, to: &Oid, limit: usize) -> Result<Vec<CommitInfo>> {
        let range = format!("{}..{}", from.0, to.0);
        let count = format!("--max-count={limit}");
        let out = self.cmd(
            &[
                "log",
                "-z",
                "--format=%H%x1f%ct%x1f%s",
                &count,
                &range,
                "--",
            ],
            true,
        )?;
        crate::repo::commit_infos(&out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::test_git;
    use std::fs;

    struct Fixture {
        _dir: crate::repo::tests::Temp,
        repo: Repo,
        root: std::path::PathBuf,
        start: Oid,
    }

    /// A repository on `main` with one commit, and a branch `work` from it.
    fn fixture(name: &str) -> Fixture {
        let (dir, git) = test_git(name);
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        repo.cmd(&["checkout", "-q", "-b", "main"], false).unwrap();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        repo.commit_changes("Start", true).unwrap();
        let start = repo.resolve("HEAD").unwrap();
        repo.cmd(&["branch", "work"], false).unwrap();
        Fixture {
            _dir: dir,
            repo,
            root,
            start,
        }
    }

    impl Fixture {
        fn commit_on(&self, branch: &str, file: &str, text: &str, subject: &str) {
            self.repo.cmd(&["checkout", "-q", branch], false).unwrap();
            fs::write(self.root.join(file), text).unwrap();
            self.repo.commit_changes(subject, true).unwrap();
            self.repo.cmd(&["checkout", "-q", "main"], false).unwrap();
        }

        fn facts(&self) -> BranchFacts {
            self.repo
                .branch_facts("work", "main", Some(&self.start), 20)
                .unwrap()
        }
    }

    #[test]
    fn a_branch_with_no_commits_has_nothing_to_merge() {
        let f = fixture("facts-nothing");
        let facts = f.facts();
        assert_eq!(facts.merge, MergeStanding::Nothing);
        assert_eq!((facts.ahead, facts.behind), (0, 0));
        assert!(facts.commits.is_empty());
    }

    #[test]
    fn a_branch_the_base_can_fast_forward_to_is_clean_and_lists_its_commits() {
        let f = fixture("facts-clean");
        f.commit_on("work", "b.txt", "b\n", "Add b");
        f.commit_on("work", "c.txt", "c\n", "Add c");
        let facts = f.facts();
        assert_eq!(facts.merge, MergeStanding::Clean);
        assert_eq!((facts.ahead, facts.behind), (2, 0));
        let subjects: Vec<&str> = facts.commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["Add c", "Add b"]);
        let capped = f.repo.branch_facts("work", "main", None, 1).unwrap();
        assert_eq!((capped.commits.len(), capped.ahead), (1, 2));
    }

    #[test]
    fn a_branch_merged_into_its_base_reads_merged() {
        let f = fixture("facts-merged");
        f.commit_on("work", "b.txt", "b\n", "Add b");
        f.repo
            .cmd(
                &["merge", "-q", "--no-ff", "-m", "Merge work", "work"],
                false,
            )
            .unwrap();
        let facts = f.facts();
        assert_eq!(facts.merge, MergeStanding::Merged);
        assert_eq!(facts.ahead, 0);
        assert_eq!(facts.behind, 1, "the merge commit");
    }

    #[test]
    fn a_clean_merge_onto_a_base_that_moved_reads_behind_with_its_count() {
        let f = fixture("facts-behind");
        f.commit_on("work", "b.txt", "b\n", "Add b");
        f.commit_on("main", "x.txt", "x\n", "Other work");
        f.commit_on("main", "y.txt", "y\n", "More other work");
        let facts = f.facts();
        assert_eq!(facts.merge, MergeStanding::Behind);
        assert_eq!((facts.ahead, facts.behind), (1, 2));
        assert!(facts.conflicts.is_empty());
    }

    #[test]
    fn a_conflicting_branch_names_its_conflicted_paths() {
        let f = fixture("facts-conflicts");
        f.commit_on("work", "a.txt", "mine\n", "Change a");
        f.commit_on("main", "a.txt", "theirs\n", "Change a too");
        let facts = f.facts();
        assert_eq!(facts.merge, MergeStanding::Conflicts);
        assert_eq!(facts.conflicts, ["a.txt"]);
        assert_eq!((facts.ahead, facts.behind), (1, 1));
    }

    #[test]
    fn uncommitted_files_and_the_base_checkout_are_read_where_they_are() {
        let f = fixture("facts-dirty");
        let tree = f.root.parent().unwrap().join("work-tree");
        f.repo
            .cmd(
                &["worktree", "add", "-q", tree.to_str().unwrap(), "work"],
                false,
            )
            .unwrap();
        let worktree = f.repo.git.open(&tree).unwrap();
        assert!(worktree.dirty_files().unwrap().is_empty());
        fs::write(tree.join("new.txt"), "new\n").unwrap();
        fs::write(tree.join("a.txt"), "edited\n").unwrap();
        assert_eq!(worktree.dirty_files().unwrap(), ["a.txt", "new.txt"]);

        assert_eq!(f.repo.checkout_of_dirty("main").unwrap(), Some(Vec::new()));
        fs::write(f.root.join("wip.txt"), "wip\n").unwrap();
        assert_eq!(
            f.repo.checkout_of_dirty("main").unwrap(),
            Some(vec!["wip.txt".to_owned()])
        );
        f.repo.cmd(&["branch", "loose"], false).unwrap();
        assert_eq!(f.repo.checkout_of_dirty("loose").unwrap(), None);
    }
}
