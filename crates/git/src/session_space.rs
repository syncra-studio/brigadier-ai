//! What a session leaves in a repository outside its own folder, so that deleting it leaves
//! nothing behind: the branches in its namespace (`<prefix><short id>/`, its own and the ones
//! its CLIs made there), their `branch.<name>.*` config, and the untracked files its turns
//! made in a Local checkout, as its turn snapshots show them.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use crate::command::{failure, valid_oid};
use crate::repo::Repo;
use crate::{Error, Oid, Result, parse};

/// What [`Repo::remove_namespace`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NamespaceRemoval {
    /// Branches deleted.
    pub deleted: Vec<String>,
    /// Branches left because a worktree has them out.
    pub checked_out: Vec<String>,
}

impl Repo {
    fn validate_namespace(&self, namespace: &str) -> Result<()> {
        let invalid = || Error::Invalid(format!("invalid branch namespace: {namespace:?}"));
        if namespace.len() < 2 || !namespace.ends_with('/') || namespace.starts_with('/') {
            return Err(invalid());
        }
        self.validate_branch(&format!("{namespace}x"))
            .map_err(|_| invalid())
    }

    /// The local branches whose names start with `namespace` (which ends with `/`), with
    /// their tips, by name.
    pub fn branches_in(&self, namespace: &str) -> Result<Vec<(String, Oid)>> {
        self.validate_namespace(namespace)?;
        let pattern = format!("refs/heads/{namespace}");
        let out = self.cmd(
            &[
                "for-each-ref",
                "--format=%(refname:strip=2)%00%(objectname)",
                &pattern,
            ],
            true,
        )?;
        let mut branches = Vec::new();
        for line in parse::text(&out)?.lines() {
            if let Some((name, tip)) = line.split_once('\0')
                && name.starts_with(namespace)
            {
                branches.push((name.to_owned(), Oid(tip.to_owned())));
            }
        }
        branches.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(branches)
    }

    /// The branches named by a `branch.<name>.*` config section that start with `namespace`,
    /// whether or not the branch itself is still there.
    pub fn branch_config_in(&self, namespace: &str) -> Result<Vec<String>> {
        self.validate_namespace(namespace)?;
        let out = self.run(
            &[
                "config",
                "--local",
                "--name-only",
                "--get-regexp",
                r"^branch\.",
            ],
            true,
        )?;
        // 1: no such key.
        if out.status.code() == Some(1) {
            return Ok(Vec::new());
        }
        if !out.status.success() {
            return Err(failure(&["config", "--get-regexp"], &out));
        }
        let mut names: Vec<String> = parse::text(&out.stdout)?
            .lines()
            .filter_map(|key| key.strip_prefix("branch.")?.rsplit_once('.'))
            .map(|(name, _)| name.to_owned())
            .filter(|name| name.starts_with(namespace))
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// Removes the `branch.<name>` config section, if there is one.
    pub fn remove_branch_config(&self, name: &str) -> Result<()> {
        self.validate_branch(name)?;
        let section = format!("branch.{name}");
        let out = self.run(&["config", "--local", "--remove-section", &section], false)?;
        // 128: no such section, which is what is wanted.
        if out.status.success() || out.status.code() == Some(128) {
            Ok(())
        } else {
            Err(failure(&["config", "--remove-section", &section], &out))
        }
    }

    /// Deletes every branch in `namespace` that no worktree has out, unmerged ones too, and
    /// every `branch.<name>.*` config section in it (those of branches left too only once they
    /// go). Under the ref lock every landing and branch delete takes.
    pub fn remove_namespace(&self, namespace: &str) -> Result<NamespaceRemoval> {
        let branches = self.branches_in(namespace)?;
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let out: Vec<String> = self
            .worktrees()?
            .into_iter()
            .filter_map(|worktree| worktree.branch)
            .collect();
        let mut removal = NamespaceRemoval::default();
        for (name, tip) in branches {
            if out.contains(&name) {
                removal.checked_out.push(name);
                continue;
            }
            self.cmd(
                &[
                    "update-ref",
                    "-m",
                    "Brigadier removed a deleted session's branch",
                    "-d",
                    &format!("refs/heads/{name}"),
                    &tip.0,
                ],
                false,
            )?;
            removal.deleted.push(name);
        }
        for name in self.branch_config_in(namespace)? {
            if !removal.checked_out.contains(&name) {
                self.remove_branch_config(&name)?;
            }
        }
        Ok(removal)
    }

    /// The files session `session`'s turns added (absent from a turn's start snapshot,
    /// present in its end one) and still has in its last end snapshot, each with its blob
    /// there. Files made between turns (by the user) are not among them.
    pub fn session_added_files(&self, session: &str) -> Result<Vec<(String, Oid)>> {
        let prefix = crate::snapshot::session_refs(session)?;
        let refs = self.cmd(
            &[
                "for-each-ref",
                "--format=%(refname)%00%(objectname)",
                &prefix,
            ],
            true,
        )?;
        // Turn number → (start tree, end tree).
        let mut turns: BTreeMap<u64, (Option<Oid>, Option<Oid>)> = BTreeMap::new();
        for line in parse::text(&refs)?.lines() {
            let Some((name, oid)) = line.split_once('\0') else {
                continue;
            };
            let Some(rest) = name
                .strip_prefix(prefix.as_str())
                .and_then(|rest| rest.strip_prefix("turn-"))
            else {
                continue;
            };
            let (turn, end) = match (rest.strip_suffix("-start"), rest.strip_suffix("-end")) {
                (Some(turn), _) => (turn, false),
                (_, Some(turn)) => (turn, true),
                _ => continue,
            };
            let Ok(turn) = turn.parse::<u64>() else {
                continue;
            };
            let entry = turns.entry(turn).or_default();
            let oid = Oid(oid.to_owned());
            if end {
                entry.1 = Some(oid);
            } else {
                entry.0 = Some(oid);
            }
        }
        let Some(last) = turns.values().rev().find_map(|(_, end)| end.clone()) else {
            return Ok(Vec::new());
        };
        let mut added = std::collections::BTreeSet::new();
        for (start, end) in turns.values() {
            let (Some(start), Some(end)) = (start, end) else {
                continue;
            };
            let out = self.cmd(
                &[
                    "diff-tree",
                    "-r",
                    "-z",
                    "--no-renames",
                    "--diff-filter=A",
                    "--name-only",
                    &start.0,
                    &end.0,
                ],
                true,
            )?;
            for path in out.split(|b| *b == 0).filter(|path| !path.is_empty()) {
                added.insert(parse::text(path)?.to_owned());
            }
        }
        let mut files = Vec::new();
        for path in added {
            let spec = format!("{}:{path}", last.0);
            let out = self.run(&["rev-parse", "--verify", "--quiet", &spec], true)?;
            if out.status.success() {
                files.push((path, parse::oid(&out.stdout)?));
            }
        }
        Ok(files)
    }

    /// Whether `path` (relative to the checkout) is a file git doesn't track whose content is
    /// still `blob`: what [`Self::remove_untracked_if`] would remove.
    pub fn untracked_unchanged(&self, path: &str, blob: &Oid) -> Result<bool> {
        valid_oid(blob)?;
        let relative = Path::new(path);
        if path.is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(Error::Invalid(format!("invalid checkout path: {path:?}")));
        }
        let full = self.root().join(relative);
        // A link, a folder, or a file under a link: never followed.
        let mut at = self.root().to_path_buf();
        for part in relative.components() {
            at.push(part);
            match std::fs::symlink_metadata(&at) {
                Ok(meta) if meta.file_type().is_symlink() => return Ok(false),
                Ok(_) => {}
                Err(_) => return Ok(false),
            }
        }
        if !std::fs::symlink_metadata(&full).is_ok_and(|meta| meta.is_file()) {
            return Ok(false);
        }
        let tracked = self.run(&["ls-files", "--error-unmatch", "--", path], true)?;
        if tracked.status.success() {
            return Ok(false);
        }
        let hashed = self.cmd(&["hash-object", "--", path], true)?;
        Ok(parse::oid(&hashed)? == *blob)
    }

    /// Removes `path` while it is a file git doesn't track whose content is still `blob`, and
    /// the folders above it that it leaves empty. `false`: it stays (changed, tracked now, or
    /// gone already).
    pub fn remove_untracked_if(&self, path: &str, blob: &Oid) -> Result<bool> {
        if !self.untracked_unchanged(path, blob)? {
            return Ok(false);
        }
        let full = self.root().join(path);
        std::fs::remove_file(&full)
            .map_err(|err| Error::Invalid(format!("{}: {err}", full.display())))?;
        let mut folder = full.parent();
        while let Some(dir) = folder {
            if dir == self.root() || std::fs::remove_dir(dir).is_err() {
                break;
            }
            folder = dir.parent();
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn fixture(name: &str) -> (crate::repo::tests::Temp, Repo) {
        let (dir, git) =
            crate::repo::tests::test_git(&format!("20261025-1221-cleanup-on-delete-w01-{name}"));
        let root = dir.join("repo");
        git.init(&root).unwrap();
        let repo = git.open(&root).unwrap();
        repo.cmd(&["checkout", "-b", "main"], false).unwrap();
        fs::write(root.join("README"), "start\n").unwrap();
        repo.commit_changes("Start", true).unwrap();
        (dir, repo)
    }

    #[test]
    fn a_namespace_goes_with_its_config_and_leaves_the_rest() {
        let (_dir, repo) = fixture("space");
        let head = repo.resolve("HEAD").unwrap();
        for name in [
            "brigadier/abcd1234/session",
            "brigadier/abcd1234/task-1-x",
            "brigadier/abcd1234/cli-made",
            "brigadier/abcd12345/other",
            "brigadier/ffff0000/session",
            "harness/w1",
        ] {
            repo.create_branch(name, &head).unwrap();
            repo.set_merge_base(name, "main").unwrap();
        }
        // A config section whose branch is gone already.
        repo.cmd(
            &[
                "config",
                "branch.brigadier/abcd1234/gone.gh-merge-base",
                "main",
            ],
            false,
        )
        .unwrap();
        let wt = repo.root().parent().unwrap().join("space-wt");
        repo.add_worktree(
            &wt,
            crate::WorktreeSpec::Branch {
                name: "brigadier/abcd1234/session".into(),
            },
        )
        .unwrap();
        let found: Vec<String> = repo
            .branches_in("brigadier/abcd1234/")
            .unwrap()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(
            found,
            [
                "brigadier/abcd1234/cli-made",
                "brigadier/abcd1234/session",
                "brigadier/abcd1234/task-1-x"
            ]
        );
        let removal = repo.remove_namespace("brigadier/abcd1234/").unwrap();
        assert_eq!(
            removal.deleted,
            ["brigadier/abcd1234/cli-made", "brigadier/abcd1234/task-1-x"]
        );
        assert_eq!(removal.checked_out, ["brigadier/abcd1234/session"]);
        assert_eq!(
            repo.branch_config_in("brigadier/abcd1234/").unwrap(),
            ["brigadier/abcd1234/session"]
        );
        repo.remove_worktree(&wt, true).unwrap();
        let removal = repo.remove_namespace("brigadier/abcd1234/").unwrap();
        assert_eq!(removal.deleted, ["brigadier/abcd1234/session"]);
        assert!(repo.branches_in("brigadier/abcd1234/").unwrap().is_empty());
        assert!(
            repo.branch_config_in("brigadier/abcd1234/")
                .unwrap()
                .is_empty()
        );
        for kept in [
            "brigadier/abcd12345/other",
            "brigadier/ffff0000/session",
            "harness/w1",
        ] {
            assert!(repo.branch_tip(kept).unwrap().is_some(), "{kept}");
            assert_eq!(repo.merge_base_of(kept).unwrap().as_deref(), Some("main"));
        }
        assert!(repo.branches_in("brigadier").is_err());
        assert!(repo.branches_in("/x/").is_err());
        // Twice is fine.
        assert_eq!(
            repo.remove_namespace("brigadier/abcd1234/").unwrap(),
            NamespaceRemoval::default()
        );
        repo.remove_branch_config("brigadier/abcd1234/none")
            .unwrap();
    }

    #[test]
    fn only_the_files_a_session_turn_added_and_left_unchanged_go() {
        let (_dir, repo) = fixture("added");
        let root = repo.root().to_path_buf();
        fs::write(root.join("before.txt"), "the user's\n").unwrap();
        repo.snapshot_turn("s1", 1, false).unwrap();
        fs::create_dir_all(root.join("shots/deep")).unwrap();
        fs::write(root.join("shots/deep/screen.png"), "png").unwrap();
        fs::write(root.join("mock.html"), "<p>mock</p>").unwrap();
        fs::write(root.join("kept.rs"), "fn main() {}\n").unwrap();
        repo.snapshot_turn("s1", 1, true).unwrap();
        // Between turns the user adds a file of their own.
        fs::write(root.join("between.txt"), "the user's too\n").unwrap();
        repo.snapshot_turn("s1", 2, false).unwrap();
        fs::write(root.join("notes.md"), "scratch").unwrap();
        // The session changes its own mockup in a later turn.
        fs::write(root.join("mock.html"), "<p>mock 2</p>").unwrap();
        repo.snapshot_turn("s1", 2, true).unwrap();
        // The session commits one; the user edits another afterwards.
        repo.cmd(&["add", "kept.rs"], false).unwrap();
        repo.commit_changes("Keep", false).unwrap();
        fs::write(root.join("notes.md"), "the user edited it").unwrap();
        let files = repo.session_added_files("s1").unwrap();
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            ["kept.rs", "mock.html", "notes.md", "shots/deep/screen.png"]
        );
        let removed: Vec<&str> = files
            .iter()
            .filter(|(path, blob)| repo.remove_untracked_if(path, blob).unwrap())
            .map(|(path, _)| path.as_str())
            .collect();
        assert_eq!(removed, ["mock.html", "shots/deep/screen.png"]);
        assert!(!root.join("shots").exists());
        for stays in ["before.txt", "between.txt", "kept.rs", "notes.md"] {
            assert!(root.join(stays).exists(), "{stays}");
        }
        assert!(repo.remove_untracked_if("../x", &files[0].1).is_err());
        assert!(repo.session_added_files("s2").unwrap().is_empty());
    }
}
