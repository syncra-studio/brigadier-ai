//! The user's own source control in a checkout: staging, unstaging and discarding files.
//! Every change takes the landing lock and is refused while the checkout is busy, as a commit
//! is. Paths go to git NUL separated on stdin and are read literally.

use std::{collections::BTreeSet, fs};

use crate::{
    Repo, Result, SourceChanges, SourceEntry, SourceKind,
    command::{check, valid_path},
    parse,
};

impl Repo {
    /// The checkout's staged and unstaged files.
    pub fn source_changes(&self) -> Result<SourceChanges> {
        parse::source_changes(&self.cmd(
            &["status", "--porcelain=v2", "-z", "--untracked-files=all"],
            true,
        )?)
    }

    /// Stages the listed changes (every change with `None`), new and deleted files included.
    pub fn stage(&self, paths: Option<&[String]>) -> Result<()> {
        self.with_source(paths, |repo, status, wanted| {
            let pathspecs = pathspecs(&status.changes, wanted);
            if !pathspecs.is_empty() {
                repo.pathspec_cmd(&["add", "-A"], &pathspecs)?;
            }
            Ok(())
        })
    }

    /// Unstages the listed staged files (every one with `None`): their index entries go back
    /// to HEAD's, or out of the index before the first commit.
    pub fn unstage(&self, paths: Option<&[String]>) -> Result<()> {
        self.with_source(paths, |repo, status, wanted| {
            let pathspecs = pathspecs(&status.staged, wanted);
            if !pathspecs.is_empty() {
                let source = format!("--source={}", repo.head_or_empty()?);
                repo.pathspec_cmd(&["restore", &source, "--staged"], &pathspecs)?;
            }
            Ok(())
        })
    }

    /// Throws away the listed changes (all of a side with `None`). Unstaged: tracked files go
    /// back to the index's version and untracked ones are deleted (never ignored ones, and
    /// never an untracked folder such as a nested repository). Staged: the index and the
    /// files go back to HEAD's version, so a file new in the index is deleted, and a rename
    /// restores its old path and deletes its new one.
    pub fn discard(&self, staged: bool, paths: Option<&[String]>) -> Result<()> {
        self.with_source(paths, |repo, status, wanted| {
            if staged {
                let pathspecs = pathspecs(&status.staged, wanted);
                if !pathspecs.is_empty() {
                    let source = format!("--source={}", repo.head_or_empty()?);
                    repo.pathspec_cmd(&["restore", &source, "--staged", "--worktree"], &pathspecs)?;
                }
                return Ok(());
            }
            let chosen: Vec<&SourceEntry> = status
                .changes
                .iter()
                .filter(|entry| picked(entry, wanted))
                .collect();
            // A conflict has no single index version: it goes back to HEAD's.
            let conflicted = pathspecs_of(
                chosen
                    .iter()
                    .copied()
                    .filter(|entry| entry.kind == SourceKind::Conflicted),
            );
            if !conflicted.is_empty() {
                let source = format!("--source={}", repo.head_or_empty()?);
                repo.pathspec_cmd(&["restore", &source, "--staged", "--worktree"], &conflicted)?;
            }
            // Only marked to be added (`git add -N`): no content of its own to go back to.
            let intended = pathspecs_of(chosen.iter().copied().filter(|entry| {
                entry.kind == SourceKind::Added
                    && !status.staged.iter().any(|staged| staged.path == entry.path)
            }));
            if !intended.is_empty() {
                repo.pathspec_cmd(&["rm", "--cached", "--force", "--quiet"], &intended)?;
            }
            let tracked = pathspecs_of(chosen.iter().copied().filter(|entry| {
                !matches!(entry.kind, SourceKind::Untracked | SourceKind::Conflicted)
                    && !intended.contains(&entry.path)
            }));
            if !tracked.is_empty() {
                repo.pathspec_cmd(&["restore", "--worktree"], &tracked)?;
            }
            for path in chosen
                .iter()
                .filter(|entry| entry.kind == SourceKind::Untracked && !entry.path.ends_with('/'))
                .map(|entry| &entry.path)
                .chain(&intended)
            {
                repo.delete_file(path)?;
            }
            Ok(())
        })
    }

    /// Runs `change` under the landing lock on a checkout that isn't busy, with its status and
    /// the validated paths asked for (`None`: all).
    fn with_source(
        &self,
        paths: Option<&[String]>,
        change: impl FnOnce(&Repo, &SourceChanges, Option<&BTreeSet<&str>>) -> Result<()>,
    ) -> Result<()> {
        let wanted = match paths {
            Some(paths) => {
                for path in paths {
                    valid_path(path)?;
                }
                Some(paths.iter().map(String::as_str).collect::<BTreeSet<_>>())
            }
            None => None,
        };
        if wanted.as_ref().is_some_and(BTreeSet::is_empty) {
            return Ok(());
        }
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(what) = self.busy()? {
            return Err(crate::Error::Invalid(format!(
                "the checkout is busy ({what})"
            )));
        }
        let status = self.source_changes()?;
        change(self, &status, wanted.as_ref())
    }

    /// HEAD's commit, or the empty tree before the first commit.
    fn head_or_empty(&self) -> Result<String> {
        let out = self.run(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"], true)?;
        if out.status.success() {
            Ok(parse::oid(&out.stdout)?.0)
        } else {
            Ok(self.empty_tree()?.0)
        }
    }

    /// Runs git with `args` and `pathspecs` given NUL separated on stdin.
    fn pathspec_cmd(&self, args: &[&str], pathspecs: &[String]) -> Result<Vec<u8>> {
        let mut command = args.to_vec();
        command.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        let mut input = Vec::new();
        for path in pathspecs {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        let output = self
            .git
            .run(Some(&self.root), &command, false, &[], Some(&input))?;
        check(&command, output)
    }

    /// Deletes an untracked file, then the folders it leaves empty.
    fn delete_file(&self, path: &str) -> Result<()> {
        valid_path(path)?;
        let full = self.root.join(path);
        match fs::symlink_metadata(&full) {
            Ok(meta) if meta.is_dir() => return Ok(()),
            Ok(_) => fs::remove_file(&full)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        let mut dir = full.parent();
        while let Some(parent) = dir {
            if parent == self.root || !parent.starts_with(&self.root) {
                break;
            }
            if fs::remove_dir(parent).is_err() {
                break;
            }
            dir = parent.parent();
        }
        Ok(())
    }
}

/// Whether `entry` is among the paths asked for (`None`: all); a rename or copy by either path.
fn picked(entry: &SourceEntry, wanted: Option<&BTreeSet<&str>>) -> bool {
    wanted.is_none_or(|wanted| {
        wanted.contains(entry.path.as_str())
            || entry
                .old_path
                .as_deref()
                .is_some_and(|old| wanted.contains(old))
    })
}

/// The pathspecs of the entries asked for.
fn pathspecs(entries: &[SourceEntry], wanted: Option<&BTreeSet<&str>>) -> Vec<String> {
    pathspecs_of(entries.iter().filter(|entry| picked(entry, wanted)))
}

/// Each entry's path, and a rename's old path (a copy's source is left alone), deduplicated
/// and without an untracked folder's trailing slash.
fn pathspecs_of<'a>(entries: impl Iterator<Item = &'a SourceEntry>) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for entry in entries {
        if entry.path.ends_with('/') {
            continue;
        }
        paths.insert(entry.path.clone());
        if entry.kind == SourceKind::Renamed
            && let Some(old) = &entry.old_path
        {
            paths.insert(old.clone());
        }
    }
    paths.into_iter().collect()
}
