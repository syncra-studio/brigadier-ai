//! Turn trees retained by session refs, independent of the user's index.

use std::collections::BTreeSet;

use crate::{DiffStat, Error, Oid, Repo, Result, command::TempIndex, parse};

/// The files of a turn, separated from changes introduced by merges.
#[derive(Debug, Clone)]
pub struct TurnDiff {
    /// This turn's own statistics.
    pub stat: DiffStat,
    /// Binary-capable patch for this turn's own files.
    pub patch: String,
    /// Changes introduced by each merged branch (branch name, else its commit id).
    pub merged: Vec<MergedDiff>,
}

/// Files a merge introduced, attributed to its source branch.
#[derive(Debug, Clone)]
pub struct MergedDiff {
    /// Local branch name, or commit id if the branch was removed.
    pub branch: String,
    /// The merged branch tip.
    pub tip: Oid,
    /// The merge commit whose first-parent diff introduced these files.
    pub merge_commit: Oid,
    /// Exact statistics of the files introduced by the merge.
    pub stat: DiffStat,
    /// Binary-capable patch of those files.
    pub patch: String,
}

/// The namespace owned by one session, validated before use.
pub fn session_refs(session: &str) -> Result<String> {
    if session.is_empty()
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err(Error::Invalid("invalid snapshot session id".into()));
    }
    Ok(format!("refs/brigadier/s/{session}/"))
}

impl Repo {
    /// Capture all non-ignored files using `git add -A` and `write-tree` in a temporary
    /// copy of the index. The user's staging and cached stat timestamps stay untouched.
    pub fn snapshot_tree(&self) -> Result<Oid> {
        let index = TempIndex::new()?;
        let real = self.git_path("index")?;
        if real.exists() {
            crate::repo::copy_index(&real, &index.path)?;
            self.index_cmd(&index, &["update-index", "--no-split-index"])?;
        } else {
            self.index_cmd(&index, &["read-tree", "--empty"])?;
        }
        self.index_cmd(&index, &["add", "-A", "--", "."])?;
        parse::oid(&self.index_cmd(&index, &["write-tree"])?)
    }

    fn head(&self) -> Result<Option<Oid>> {
        let out = self.run(&["rev-parse", "--verify", "--quiet", "HEAD"], true)?;
        if out.status.success() {
            Ok(Some(parse::oid(&out.stdout)?))
        } else if out.status.code() == Some(1) {
            Ok(None)
        } else {
            crate::command::check(&["rev-parse", "HEAD"], out).map(|_| None)
        }
    }

    fn turn_ref(session: &str, turn: u64, end: bool) -> Result<String> {
        Ok(format!(
            "{}turn-{turn}-{}",
            session_refs(session)?,
            if end { "end" } else { "start" }
        ))
    }

    /// Retain a turn's start/end tree and its corresponding history anchor. Existing captures
    /// are immutable, so a reconnect cannot replace the start of a running turn.
    pub fn snapshot_turn(&self, session: &str, turn: u64, end: bool) -> Result<Oid> {
        let name = Self::turn_ref(session, turn, end)?;
        if let Some(tree) = self.snapshot_ref(&name)? {
            return Ok(tree);
        }
        let tree = self.snapshot_tree()?;
        let head = self.head()?;
        let mut input = format!("start\ncreate {name} {}\n", tree.0);
        if let Some(head) = head {
            input.push_str(&format!("create {name}-head {}\n", head.0));
        }
        input.push_str("prepare\ncommit\n");
        self.git.checked(
            Some(&self.root),
            &["update-ref", "--stdin"],
            false,
            &[],
            Some(input.as_bytes()),
        )?;
        Ok(tree)
    }

    fn snapshot_ref(&self, name: &str) -> Result<Option<Oid>> {
        let out = self.run(&["show-ref", "--verify", "--hash", name], true)?;
        if out.status.success() {
            Ok(Some(parse::oid(&out.stdout)?))
        } else if out.status.code() == Some(1) || out.status.code() == Some(128) {
            Ok(None)
        } else {
            crate::command::check(&["show-ref", name], out).map(|_| None)
        }
    }

    /// Read a retained turn tree; no checkout or index access is needed.
    pub fn turn_tree(&self, session: &str, turn: u64, end: bool) -> Result<Oid> {
        let name = Self::turn_ref(session, turn, end)?;
        self.snapshot_ref(&name)?
            .ok_or_else(|| Error::Invalid(format!("missing snapshot {name}")))
    }

    /// Exact git diff, with the paths introduced by merge commits attributed to their branches.
    pub fn turn_diff(&self, session: &str, turn: u64) -> Result<TurnDiff> {
        let start = self.turn_tree(session, turn, false)?;
        let end = self.turn_tree(session, turn, true)?;
        let first =
            self.snapshot_ref(&format!("{}-head", Self::turn_ref(session, turn, false)?))?;
        let last = self.snapshot_ref(&format!("{}-head", Self::turn_ref(session, turn, true)?))?;
        let mut excluded = BTreeSet::new();
        let mut merged = Vec::new();
        if let (Some(first), Some(last)) = (first, last) {
            let range = format!("{}..{}", first.0, last.0);
            let commits = self.cmd(
                &[
                    "rev-list",
                    "--first-parent",
                    "--merges",
                    "--reverse",
                    "--parents",
                    &range,
                ],
                true,
            )?;
            for line in parse::text(&commits)?.lines() {
                let ids: Vec<_> = line.split_whitespace().collect();
                let merge = Oid(ids[0].into());
                let parent = Oid(ids[1].into());
                let merge_paths = self.diff_scope(&parent, &merge)?.1;
                excluded.extend(merge_paths.iter().cloned());
                let mut assigned = BTreeSet::new();
                for source in &ids[2..] {
                    let tip = Oid((*source).into());
                    let base = self.merge_base(&parent, &tip)?;
                    let side_paths = self.diff_scope(&base, &tip)?.1;
                    let paths: Vec<_> = merge_paths
                        .iter()
                        .filter(|path| {
                            (side_paths.contains(path) || Some(source) == ids.last())
                                && !assigned.contains(*path)
                        })
                        .cloned()
                        .collect();
                    assigned.extend(paths.iter().cloned());
                    let stat = filter_stat(self.diff_stat(&parent, &merge)?, &paths);
                    let branches = self.cmd(
                        &[
                            "for-each-ref",
                            "--format=%(refname:short)",
                            &format!("--points-at={source}"),
                            "refs/heads/",
                        ],
                        true,
                    )?;
                    let branch = parse::text(&branches)?
                        .lines()
                        .next()
                        .unwrap_or(source)
                        .to_owned();
                    merged.push(MergedDiff {
                        branch,
                        tip,
                        merge_commit: merge.clone(),
                        stat,
                        patch: self.diff_paths(&parent, &merge, &paths)?,
                    });
                }
            }
        }
        let stat = self.diff_stat(&start, &end)?;
        let paths: Vec<_> = self
            .diff_scope(&start, &end)?
            .1
            .into_iter()
            .filter(|path| !excluded.contains(path))
            .collect();
        let stat = filter_stat(stat, &paths);
        let patch = self.diff_paths(&start, &end, &paths)?;
        Ok(TurnDiff {
            stat,
            patch,
            merged,
        })
    }

    fn diff_paths(&self, from: &Oid, to: &Oid, paths: &[String]) -> Result<String> {
        if paths.is_empty() {
            return Ok(String::new());
        }
        let mut args = vec![
            "--literal-pathspecs",
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--binary",
            "--full-index",
            "--find-renames",
            &from.0,
            &to.0,
            "--",
        ];
        args.extend(paths.iter().map(String::as_str));
        Ok(parse::text(&self.cmd(&args, true)?)?.to_owned())
    }

    /// A worker's current files against the parent's current branch, including untracked work.
    pub fn worker_diff(&self, parent_branch: &str) -> Result<DiffStat> {
        let parent = self.branch_commit(parent_branch)?;
        self.diff_stat(&parent, &self.snapshot_tree()?)
    }

    /// Files changed after the turn, to name before asking to restore the start tree.
    pub fn changes_after_turn(&self, session: &str, turn: u64) -> Result<Vec<String>> {
        Ok(self
            .diff_scope(
                &self.turn_tree(session, turn, true)?,
                &self.snapshot_tree()?,
            )?
            .1)
    }

    /// Restore the complete start tree, after the caller's confirmation. The expected current
    /// tree prevents overwriting edits made while the confirmation was open. HEAD and the real
    /// index stay untouched; ignored files are not removed.
    pub fn revert_turn(&self, session: &str, turn: u64, expected: &Oid) -> Result<Vec<String>> {
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(what) = self.busy()? {
            return Err(Error::Invalid(format!("checkout is busy ({what})")));
        }
        let current = self.snapshot_tree()?;
        if &current != expected {
            return Err(Error::Invalid(
                "files changed since confirmation; review them again".into(),
            ));
        }
        let target = self.turn_tree(session, turn, false)?;
        let paths = self.diff_scope(&current, &target)?.1;
        let index = TempIndex::new()?;
        self.index_cmd(&index, &["read-tree", &current.0])?;
        self.index_cmd(&index, &["update-index", "--refresh"])?;
        // Two-tree checkout refuses untracked/ignored collisions rather than deleting them.
        self.index_cmd(&index, &["read-tree", "-m", "-u", &current.0, &target.0])?;
        Ok(paths)
    }

    /// The file review's two trees, initially the turn's immutable snapshots. Keep advances
    /// only the review base; file Undo advances only the review end. The card never reads these.
    pub fn turn_review_trees(&self, session: &str, turn: u64) -> Result<(Oid, Oid)> {
        let prefix = format!("{}turn-{turn}-review", session_refs(session)?);
        Ok((
            self.snapshot_ref(&format!("{prefix}-base"))?
                .unwrap_or(self.turn_tree(session, turn, false)?),
            self.snapshot_ref(&format!("{prefix}-end"))?
                .unwrap_or(self.turn_tree(session, turn, true)?),
        ))
    }

    /// Update one review file in git, without modifying the immutable turn trees.
    pub fn review_turn_file(
        &self,
        session: &str,
        turn: u64,
        keep: bool,
        path: &str,
        blob: Option<crate::FileBlob>,
    ) -> Result<()> {
        let (base, end) = self.turn_review_trees(session, turn)?;
        let tree = self.blob_tree(
            Some(if keep { &base } else { &end }),
            &std::collections::BTreeMap::from([(path.to_owned(), blob)]),
        )?;
        let name = format!(
            "{}turn-{turn}-review-{}",
            session_refs(session)?,
            if keep { "base" } else { "end" }
        );
        self.cmd(&["update-ref", &name, &tree.0], false)?;
        Ok(())
    }

    /// Delete precisely a session's ref namespace. Archive deliberately never calls this.
    pub fn delete_session_snapshots(&self, session: &str) -> Result<()> {
        let prefix = session_refs(session)?;
        let refs = self.cmd(&["for-each-ref", "--format=%(refname)", &prefix], true)?;
        let mut input = String::from("start\n");
        for name in parse::text(&refs)?.lines() {
            input.push_str(&format!("delete {name}\n"));
        }
        input.push_str("prepare\ncommit\n");
        self.git.checked(
            Some(&self.root),
            &["update-ref", "--stdin"],
            false,
            &[],
            Some(input.as_bytes()),
        )?;
        Ok(())
    }
}

fn filter_stat(mut stat: DiffStat, paths: &[String]) -> DiffStat {
    stat.files.retain(|file| paths.contains(&file.path));
    stat.insertions = stat.files.iter().map(|f| f.insertions).sum();
    stat.deletions = stat.files.iter().map(|f| f.deletions).sum();
    stat
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::tests::test_git;
    use std::fs;

    fn fixture(name: &str) -> (crate::repo::tests::Temp, Repo) {
        let (dir, git) = test_git(&format!("20261022-0805-take-over-5-sessions-w45-{name}"));
        let root = dir.join("repo");
        git.init(&root).unwrap();
        let repo = git.open(&root).unwrap();
        repo.cmd(&["checkout", "-b", "main"], false).unwrap();
        fs::write(root.join("delete"), "remove\n").unwrap();
        fs::write(root.join("rename"), "rename this unchanged file\n").unwrap();
        fs::write(root.join("binary"), [0, 1, 2]).unwrap();
        fs::write(root.join(".gitignore"), "ignored\n").unwrap();
        repo.commit_changes("Start", true).unwrap();
        (dir, repo)
    }

    #[test]
    fn captures_new_deleted_renamed_binary_untracked_and_unchanged() {
        let (_dir, repo) = fixture("types");
        let index = fs::read(repo.git_dir().unwrap().join("index")).unwrap();
        let start = repo.snapshot_turn("s1", 1, false).unwrap();
        fs::remove_file(repo.root.join("delete")).unwrap();
        fs::rename(repo.root.join("rename"), repo.root.join("renamed")).unwrap();
        fs::write(repo.root.join("binary"), [0, 3, 4, 5]).unwrap();
        fs::write(repo.root.join("new"), "new\n").unwrap();
        repo.cmd(&["add", "new"], false).unwrap();
        fs::write(repo.root.join("untracked"), "untracked\n").unwrap();
        fs::write(repo.root.join("ignored"), "invisible\n").unwrap();
        let staged = fs::read(repo.git_dir().unwrap().join("index")).unwrap();
        repo.snapshot_turn("s1", 1, true).unwrap();
        assert_eq!(
            fs::read(repo.git_dir().unwrap().join("index")).unwrap(),
            staged
        );
        assert_ne!(index, staged);
        assert_eq!(repo.snapshot_turn("s1", 1, false).unwrap(), start);
        let diff = repo.turn_diff("s1", 1).unwrap();
        let paths: Vec<_> = diff.stat.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["binary", "delete", "new", "renamed", "untracked"]);
        assert!(diff.stat.files[0].binary);
        assert!(diff.patch.contains("rename from rename"), "{}", diff.patch);
        assert!(diff.patch.contains("GIT binary patch"));
        repo.snapshot_turn("s1", 2, false).unwrap();
        repo.snapshot_turn("s1", 2, true).unwrap();
        assert!(repo.turn_diff("s1", 2).unwrap().stat.files.is_empty());
        assert!(repo.turn_diff("s1", 2).unwrap().patch.is_empty());
    }

    #[test]
    fn revert_restores_start_tree_and_names_later_files() {
        let (_dir, repo) = fixture("revert");
        fs::write(repo.root.join("preexisting"), "dirty before start\n").unwrap();
        let start = repo.snapshot_turn("s1", 1, false).unwrap();
        fs::write(repo.root.join("new"), "turn\n").unwrap();
        fs::remove_file(repo.root.join("delete")).unwrap();
        repo.commit_changes("Turn", true).unwrap();
        repo.snapshot_turn("s1", 1, true).unwrap();
        fs::write(repo.root.join("later"), "after\n").unwrap();
        assert_eq!(repo.changes_after_turn("s1", 1).unwrap(), ["later"]);
        let expected = repo.snapshot_tree().unwrap();
        fs::write(repo.root.join("later"), "raced\n").unwrap();
        assert!(repo.revert_turn("s1", 1, &expected).is_err());
        let expected = repo.snapshot_tree().unwrap();
        let index = fs::read(repo.git_dir().unwrap().join("index")).unwrap();
        let head = repo.resolve("HEAD").unwrap();
        repo.revert_turn("s1", 1, &expected).unwrap();
        assert_eq!(repo.snapshot_tree().unwrap(), start);
        assert!(
            repo.diff(&start, &repo.snapshot_tree().unwrap())
                .unwrap()
                .is_empty()
        );
        assert_eq!(repo.resolve("HEAD").unwrap(), head);
        assert_eq!(
            fs::read(repo.git_dir().unwrap().join("index")).unwrap(),
            index
        );
    }

    #[test]
    fn retained_refs_survive_worktree_removal_and_delete_is_session_scoped() {
        let (_dir, repo) = fixture("cleanup");
        repo.snapshot_turn("s1", 1, false).unwrap();
        repo.snapshot_turn("s1", 1, true).unwrap();
        repo.snapshot_turn("s2", 1, false).unwrap();
        // Archive preserves refs; reopening and gc must still read the exact card.
        repo.cmd(&["gc", "--prune=now"], false).unwrap();
        assert!(repo.turn_diff("s1", 1).unwrap().stat.files.is_empty());
        repo.delete_session_snapshots("s1").unwrap();
        assert!(
            repo.read_only(&["for-each-ref", "refs/brigadier/s/s1/"])
                .unwrap()
                .is_empty()
        );
        assert!(repo.turn_tree("s2", 1, false).is_ok());
        assert!(session_refs("../s2").is_err());
    }

    #[test]
    fn merged_branch_files_leave_parent_card_and_worker_counts_use_parent() {
        let (_dir, repo) = fixture("merge");
        let base = repo.resolve("HEAD").unwrap();
        repo.cmd(&["checkout", "-b", "worker"], false).unwrap();
        fs::write(repo.root.join("worker"), "one\ntwo\n").unwrap();
        repo.commit_changes("Worker", true).unwrap();
        assert_eq!(repo.worker_diff("main").unwrap().insertions, 2);
        fs::write(repo.root.join("worker-dirty"), "three\n").unwrap();
        assert_eq!(repo.worker_diff("main").unwrap().insertions, 3);
        fs::remove_file(repo.root.join("worker-dirty")).unwrap();
        repo.cmd(&["checkout", "main"], false).unwrap();
        repo.snapshot_turn("s1", 1, false).unwrap();
        fs::write(repo.root.join("own"), "own\n").unwrap();
        repo.commit_changes("Own", true).unwrap();
        repo.cmd(&["merge", "--no-ff", "worker", "-m", "Merge worker"], false)
            .unwrap();
        repo.snapshot_turn("s1", 1, true).unwrap();
        let diff = repo.turn_diff("s1", 1).unwrap();
        assert_eq!(
            diff.stat
                .files
                .iter()
                .map(|f| f.path.as_str())
                .collect::<Vec<_>>(),
            ["own"]
        );
        assert_eq!(diff.merged.len(), 1);
        assert_eq!(diff.merged[0].branch, "worker");
        assert_eq!(diff.merged[0].stat.insertions, 2);
        assert!(diff.merged[0].patch.contains("+two"));
        assert!(!diff.patch.contains("+two"));
        assert_ne!(base, repo.resolve("HEAD").unwrap());
    }

    #[test]
    fn unborn_repository_can_snapshot_and_revert() {
        let (dir, git) = test_git("20261022-0805-take-over-5-sessions-w45-unborn");
        let root = dir.join("repo");
        git.init(&root).unwrap();
        let repo = git.open(&root).unwrap();
        let start = repo.snapshot_turn("s1", 0, false).unwrap();
        fs::write(root.join("new"), "hello\n").unwrap();
        repo.snapshot_turn("s1", 0, true).unwrap();
        repo.revert_turn("s1", 0, &repo.snapshot_tree().unwrap())
            .unwrap();
        assert_eq!(repo.snapshot_tree().unwrap(), start);
    }
}
