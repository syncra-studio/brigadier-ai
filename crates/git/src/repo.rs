use crate::{
    Branch, Change, ChangeKind, CheckoutTrees, CollidingPath, CommitInfo, DiffStat, Environment,
    Error, Git, LandBlock, LandOutcome, LandRequest, MergeAudit, MergeOutcome, Oid, PatchOutcome,
    RemoteState, RepoState, Result, RevertOutcome, Side, Snapshot, Vanished, Worktree,
    WorktreeInfo, WorktreeSpec,
    command::{TempIndex, check, failure, valid_oid, valid_path},
    parse,
};
use std::{
    collections::{BTreeSet, HashMap},
    ffi::{OsStr, OsString},
    fs,
    path::{Path, PathBuf},
    process::Output,
    sync::{Arc, Mutex, OnceLock, Weak},
};

/// A repository rooted at the user's supplied checkout, with shared worktree metadata.
#[derive(Debug, Clone)]
pub struct Repo {
    pub(crate) git: Git,
    pub(crate) root: PathBuf,
    pub(crate) common_dir: PathBuf,
}

pub(crate) enum TreeMerge {
    Ready(Oid),
    Conflicts(Vec<String>),
}

impl Repo {
    /// The canonical top-level path of this checkout.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The canonical git folder shared by the checkout and its worktrees.
    pub fn common_dir(&self) -> &Path {
        &self.common_dir
    }

    /// This checkout's own git folder: the common one for the main checkout,
    /// `<common>/worktrees/<name>` (its index and HEAD) for a linked worktree.
    pub fn git_dir(&self) -> Result<PathBuf> {
        let args = ["rev-parse", "--path-format=absolute", "--git-dir"];
        let out = check(&args, self.run(&args, true)?)?;
        Ok(fs::canonicalize(parse::path_line(&out)?)?)
    }

    pub(crate) fn run<S: AsRef<OsStr>>(&self, args: &[S], read_only: bool) -> Result<Output> {
        self.git.run(Some(&self.root), args, read_only, &[], None)
    }
    /// What a read-only git command prints in the checkout, for a caller's own look at it.
    pub fn read_only(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.cmd(args, true)?).into_owned())
    }
    pub(crate) fn cmd<S: AsRef<OsStr>>(&self, args: &[S], read_only: bool) -> Result<Vec<u8>> {
        check(args, self.run(args, read_only)?)
    }
    pub(crate) fn index_cmd(&self, index: &TempIndex, args: &[&str]) -> Result<Vec<u8>> {
        let mut command = vec!["-c", "core.splitIndex=false"];
        command.extend_from_slice(args);
        self.git
            .checked(Some(&self.root), &command, false, &index.env(), None)
    }
    pub(crate) fn status(&self, ignored: bool) -> Result<parse::Status> {
        let mut args = vec![
            "status",
            "--porcelain=v2",
            "-z",
            "--branch",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ];
        if ignored {
            args.push("--ignored=matching");
        }
        parse::status(&self.cmd(&args, true)?)
    }

    /// Query checkout status and local branches with one status and one for-each-ref process.
    pub fn state(&self) -> Result<RepoState> {
        let status = self.status(false)?;
        let bytes = self.cmd(
            &[
                "for-each-ref",
                "--sort=refname",
                "--format=%(refname)%00%(objectname)%00%(worktreepath)%00",
                "refs/heads/",
            ],
            true,
        )?;
        let mut fields = parse::fields(&bytes);
        let mut branches = Vec::new();
        while let Some(name) = fields.next() {
            let name = name.strip_prefix(b"\n").unwrap_or(name);
            if name.is_empty() {
                continue;
            }
            let name = parse::text(name)?
                .strip_prefix("refs/heads/")
                .ok_or_else(|| Error::Parse("unexpected branch ref".into()))?
                .to_owned();
            let commit = parse::oid(
                fields
                    .next()
                    .ok_or_else(|| Error::Parse("missing branch oid".into()))?,
            )?;
            let path = fields
                .next()
                .ok_or_else(|| Error::Parse("missing branch worktree".into()))?;
            let checked_out_at = if path.is_empty() {
                None
            } else {
                Some(PathBuf::from(parse::text(path)?))
            };
            branches.push(Branch {
                name,
                commit,
                checked_out_at,
            });
        }
        Ok(RepoState {
            current_branch: status.branch.clone(),
            head: status.head.clone(),
            branches,
            dirty_files: status.dirty(),
        })
    }

    /// Resolve a revision as a commit, rejecting option injection and non-commit objects.
    pub fn resolve(&self, rev: &str) -> Result<Oid> {
        parse::oid(&self.cmd(
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{rev}^{{commit}}"),
            ],
            true,
        )?)
    }

    /// The tree a commit holds, rejecting option injection and non-commit objects. Two commits
    /// with the same tree hold the same files.
    pub fn tree_of(&self, rev: &str) -> Result<Oid> {
        parse::oid(&self.cmd(
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{rev}^{{commit}}^{{tree}}"),
            ],
            true,
        )?)
    }

    pub(crate) fn validate_branch(&self, name: &str) -> Result<()> {
        if name.starts_with('-') || name.contains('\0') {
            return Err(Error::Invalid("invalid branch name".into()));
        }
        let args = ["check-ref-format", "--branch", name];
        let out = self.run(&args, true)?;
        if !out.status.success() || parse::line(&out.stdout)? != name {
            return Err(Error::Invalid(format!(
                "invalid literal branch name: {name:?}"
            )));
        }
        Ok(())
    }

    /// Return a local branch's commit, or None if the branch does not exist.
    pub fn branch_tip(&self, branch: &str) -> Result<Option<Oid>> {
        self.validate_branch(branch)?;
        let name = format!("refs/heads/{branch}");
        let args = [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &name,
        ];
        let out = self.run(&args, true)?;
        match out.status.code() {
            Some(0) => Ok(Some(parse::oid(&out.stdout)?)),
            Some(1) => Ok(None),
            _ => Err(failure(&args, &out)),
        }
    }

    /// A local branch's commit, which must exist. Unlike `resolve`, a tag or other ref with the
    /// same name is never picked instead.
    pub fn branch_commit(&self, branch: &str) -> Result<Oid> {
        self.branch_tip(branch)?
            .ok_or_else(|| Error::Invalid(format!("there is no branch {branch:?}")))
    }

    /// Create a validated new branch at a commit; never replace an existing branch.
    pub fn create_branch(&self, name: &str, start: &Oid) -> Result<()> {
        self.validate_branch(name)?;
        valid_oid(start)?;
        self.cmd(&["branch", "--no-track", "--", name, &start.0], false)?;
        Ok(())
    }

    /// Runs `work` holding the repository's ref lock, the one every landing and branch delete
    /// takes: the engine's ref check sees no landing half done. `work` must not land or delete.
    pub fn with_ref_lock<R>(&self, work: impl FnOnce() -> R) -> R {
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        work()
    }

    /// Moves `name` back to `to` while it still points at `expected` (a branch rewritten
    /// outside Brigadier, restored by the ref check).
    pub fn reset_branch(&self, name: &str, to: &Oid, expected: &Oid) -> Result<()> {
        self.validate_branch(name)?;
        valid_oid(to)?;
        valid_oid(expected)?;
        self.cmd(
            &[
                "update-ref",
                "-m",
                "Brigadier restored a branch",
                &format!("refs/heads/{name}"),
                &to.0,
                &expected.0,
            ],
            false,
        )?;
        Ok(())
    }

    /// Delete a branch, refusing any branch currently held by a worktree, even with force.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<()> {
        self.validate_branch(name)?;
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if self
            .worktrees()?
            .iter()
            .any(|w| w.branch.as_deref() == Some(name))
        {
            return Err(Error::Invalid(format!("branch {name:?} is checked out")));
        }
        self.cmd(
            &["branch", if force { "-D" } else { "-d" }, "--", name],
            false,
        )?;
        Ok(())
    }

    /// Delete a branch only while it still points at `expected`, refusing any branch a
    /// worktree holds.
    pub fn delete_branch_at(&self, name: &str, expected: &Oid) -> Result<()> {
        self.validate_branch(name)?;
        valid_oid(expected)?;
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if self
            .worktrees()?
            .iter()
            .any(|w| w.branch.as_deref() == Some(name))
        {
            return Err(Error::Invalid(format!("branch {name:?} is checked out")));
        }
        self.cmd(
            &[
                "update-ref",
                "-m",
                "Brigadier removed a task branch",
                "-d",
                &format!("refs/heads/{name}"),
                &expected.0,
            ],
            false,
        )?;
        Ok(())
    }

    /// Create the new branch `name` at one commit of `patch` on `start` (three-way where the
    /// patch's original files are known), with the user's identity. The patch is applied in a
    /// private index: no checkout changes, and nothing is created unless all of it applies.
    pub fn branch_from_patch(
        &self,
        name: &str,
        start: &Oid,
        patch: &[u8],
        message: &str,
    ) -> Result<PatchOutcome> {
        self.validate_branch(name)?;
        valid_oid(start)?;
        if self.branch_tip(name)?.is_some() {
            return Err(Error::Invalid(format!("branch {name:?} already exists")));
        }
        let index = TempIndex::new()?;
        self.index_cmd(&index, &["read-tree", &start.0])?;
        let args = [
            "-c",
            "core.splitIndex=false",
            "apply",
            "--cached",
            "--3way",
            "--whitespace=nowarn",
            "-",
        ];
        let out = self
            .git
            .run(Some(&self.root), &args, false, &index.env(), Some(patch))?;
        if !out.status.success() {
            let unmerged = self.index_cmd(&index, &["ls-files", "--unmerged", "-z"])?;
            let paths = parse::fields(&unmerged)
                .filter_map(|entry| entry.splitn(2, |&c| c == b'\t').nth(1))
                .map(|path| parse::text(path).map(str::to_owned))
                .collect::<Result<BTreeSet<_>>>()?;
            if paths.is_empty() {
                return Ok(PatchOutcome::Failed {
                    reason: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
                });
            }
            return Ok(PatchOutcome::Conflicts {
                paths: paths.into_iter().collect(),
            });
        }
        let tree = parse::oid(&self.index_cmd(&index, &["write-tree"])?)?;
        let commit = self.commit_tree(&tree, &[start], message, false)?;
        self.create_branch(name, &commit)?;
        Ok(PatchOutcome::Applied { commit })
    }

    /// Whether all commits reachable from local branch `branch` are also reachable from local
    /// branch `into`.
    pub fn is_merged(&self, branch: &str, into: &str) -> Result<bool> {
        self.ancestor(&self.branch_commit(branch)?, &self.branch_commit(into)?)
    }
    /// Whether `from` is reachable from `to` (including equality).
    pub fn ancestor(&self, from: &Oid, to: &Oid) -> Result<bool> {
        valid_oid(from)?;
        valid_oid(to)?;
        let args = ["merge-base", "--is-ancestor", &from.0, &to.0];
        let out = self.run(&args, true)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failure(&args, &out)),
        }
    }

    /// Capture tracked and non-ignored untracked content in an unreferenced snapshot commit.
    /// The real index, working tree, stash and all refs remain untouched. An unborn dirty
    /// repository cannot supply Snapshot.head and returns Invalid.
    pub fn snapshot_uncommitted(&self) -> Result<Option<Snapshot>> {
        let status = self.status(false)?;
        let files = status.dirty();
        if files.is_empty() {
            return Ok(None);
        }
        let head = status
            .head
            .ok_or_else(|| Error::Invalid("cannot snapshot an unborn repository".into()))?;
        let (_, tree) = self.capture()?;
        let commit = self.commit_tree(&tree, &[&head], "Brigadier uncommitted snapshot", true)?;
        Ok(Some(Snapshot {
            commit,
            head,
            files,
        }))
    }

    /// The checkout's files as they stand (HEAD plus every non-ignored change, staged or not),
    /// as a commit no ref names: HEAD itself when nothing differs from it. The same files on
    /// the same HEAD give the same commit, so a snapshot can be compared with an earlier one.
    /// The real index, working tree, stash and refs remain untouched.
    pub fn snapshot_files(&self) -> Result<Oid> {
        let head = self.resolve("HEAD")?;
        let tree = match self.capture() {
            Ok((_, tree)) => tree,
            // Unresolved entries or a dirty submodule: the files as they stand all the same.
            Err(_) => self.files_tree()?,
        };
        if tree == self.tree_of(&head.0)? {
            return Ok(head);
        }
        self.commit_tree_with(
            &tree,
            &[&head],
            "Brigadier snapshot of the checkout's files",
            &fixed_identity(),
        )
    }

    /// A commit holding `tip`'s files whose parent is `base`, so `base...it` (a diff from the
    /// merge base, as reviewers read one) shows exactly what changed from `base` to `tip`:
    /// `tip` itself when `base` is its ancestor. Two snapshots of a checkout are siblings on
    /// its HEAD, and the later one's own merge-base diff would show changes the earlier one
    /// already held. No ref names it; the same pair gives the same commit.
    pub fn on_base(&self, base: &Oid, tip: &Oid) -> Result<Oid> {
        if self.ancestor(base, tip)? {
            return Ok(tip.clone());
        }
        let tree = self.tree_of(&tip.0)?;
        self.commit_tree_with(
            &tree,
            &[base],
            "Brigadier change from one snapshot to the next",
            &fixed_identity(),
        )
    }

    pub(crate) fn git_path(&self, name: &str) -> Result<PathBuf> {
        parse::path_line(&self.cmd(
            &["rev-parse", "--path-format=absolute", "--git-path", name],
            true,
        )?)
    }

    pub(crate) fn capture(&self) -> Result<(TempIndex, Oid)> {
        let status = self.status(false)?;
        if status.dirty_submodule {
            return Err(Error::Invalid("commit or discard changes inside submodules before capturing the parent repository".into()));
        }
        if status.unmerged {
            return Err(Error::Invalid(
                "checkout has unresolved index entries".into(),
            ));
        }
        let temp = TempIndex::new()?;
        let index = self.git_path("index")?;
        if index.exists() {
            copy_index(&index, &temp.path)?;
        } else {
            self.index_cmd(&temp, &["read-tree", "--empty"])?;
        }
        // Copying the real index retains staged additions even if they are now ignored.
        // Disable split-index for the private copy so no shared-index files are leaked.
        self.index_cmd(
            &temp,
            &[
                "-c",
                "core.splitIndex=false",
                "update-index",
                "--no-split-index",
            ],
        )?;
        self.index_cmd(
            &temp,
            &["-c", "core.splitIndex=false", "add", "-A", "--", "."],
        )?;
        let tree = parse::oid(&self.index_cmd(&temp, &["write-tree"])?)?;
        Ok((temp, tree))
    }

    /// The tree of the checkout's files as they stand (HEAD plus every non-ignored change),
    /// read past an index with unresolved entries, which `capture` refuses.
    pub fn files_tree(&self) -> Result<Oid> {
        let temp = TempIndex::new()?;
        self.index_cmd(&temp, &["read-tree", "HEAD"])?;
        self.index_cmd(&temp, &["add", "-A", "--", "."])?;
        parse::oid(&self.index_cmd(&temp, &["write-tree"])?)
    }

    pub(crate) fn commit_tree(
        &self,
        tree: &Oid,
        parents: &[&Oid],
        message: &str,
        internal: bool,
    ) -> Result<Oid> {
        let env: Environment = if internal {
            ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"]
                .map(|key| (key.into(), "Brigadier".into()))
                .into_iter()
                .chain(
                    ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"]
                        .map(|key| (key.into(), "brigadier@localhost".into())),
                )
                .collect()
        } else {
            vec![]
        };
        self.commit_tree_with(tree, parents, message, &env)
    }

    /// [`Repo::commit_tree`] with extra environment, such as an original author and date.
    pub(crate) fn commit_tree_with(
        &self,
        tree: &Oid,
        parents: &[&Oid],
        message: &str,
        env: &[(OsString, OsString)],
    ) -> Result<Oid> {
        valid_oid(tree)?;
        let mut args = vec!["commit-tree", &tree.0];
        for parent in parents {
            valid_oid(parent)?;
            args.extend(["-p", &parent.0]);
        }
        args.extend(["-F", "-"]);
        parse::oid(&self.git.checked(
            Some(&self.root),
            &args,
            false,
            env,
            Some(message.as_bytes()),
        )?)
    }

    /// Create a worktree at the supplied path using a new branch, existing branch or detached HEAD.
    pub fn add_worktree(&self, path: &Path, spec: WorktreeSpec) -> Result<Worktree> {
        let path = absolute(path)?;
        let mut args: Vec<OsString> = vec!["worktree".into(), "add".into()];
        match spec {
            WorktreeSpec::NewBranch { name, start } => {
                self.validate_branch(&name)?;
                valid_oid(&start)?;
                args.extend([
                    "-b".into(),
                    name.into(),
                    "--".into(),
                    path.as_os_str().to_owned(),
                    start.0.into(),
                ]);
            }
            WorktreeSpec::Branch { name } => {
                self.validate_branch(&name)?;
                if self.branch_tip(&name)?.is_none() {
                    return Err(Error::Invalid(format!("branch {name:?} does not exist")));
                }
                args.extend(["--".into(), path.as_os_str().to_owned(), name.into()]);
            }
            WorktreeSpec::Detached { at } => {
                valid_oid(&at)?;
                args.extend([
                    "--detach".into(),
                    "--".into(),
                    path.as_os_str().to_owned(),
                    at.0.into(),
                ]);
            }
        }
        self.cmd(&args, false)?;
        self.git.open_worktree(&path)
    }

    /// Remove a supplied linked worktree and prune stale metadata. Missing paths are idempotent.
    /// The caller must pass only paths in its cleanup ledger. The main checkout is never removed.
    pub fn remove_worktree(&self, path: &Path, force: bool) -> Result<()> {
        let path = absolute(path)?;
        if path == self.root || fs::canonicalize(&path).is_ok_and(|p| p == self.root) {
            return Err(Error::Invalid(
                "cannot remove the repository's own checkout".into(),
            ));
        }
        if path.try_exists()? {
            let registered = self.worktrees()?.iter().any(|w| {
                w.path == path || fs::canonicalize(&w.path).ok() == fs::canonicalize(&path).ok()
            });
            if !registered {
                return Err(Error::Invalid("path is not a registered worktree".into()));
            }
            let mut args: Vec<OsString> = vec!["worktree".into(), "remove".into()];
            if force {
                args.push("--force".into());
            }
            args.extend(["--".into(), path.into_os_string()]);
            self.cmd(&args, false)?;
        }
        self.cmd(&["worktree", "prune", "--expire=now"], false)?;
        Ok(())
    }

    /// Remove the records of worktrees whose folders are gone (`git worktree prune`).
    pub fn prune_worktrees(&self) -> Result<()> {
        self.cmd(&["worktree", "prune", "--expire=now"], false)?;
        Ok(())
    }

    /// List registered worktrees using NUL-delimited porcelain metadata.
    pub fn worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        parse::worktrees(&self.cmd(&["worktree", "list", "--porcelain", "-z"], true)?)
    }

    /// The checkout's files, tracked and untracked but not ignored, relative to its root and
    /// sorted: at most `limit`, and whether there were more. With `query`, only the paths
    /// that hold its letters in order, ignoring case.
    pub fn files(&self, limit: usize, query: Option<&str>) -> Result<(Vec<String>, bool)> {
        let out = self.cmd(
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
            true,
        )?;
        let mut files: Vec<String> = out
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .filter(|path| query.is_none_or(|query| has_letters(path, query)))
            .collect();
        files.sort();
        files.dedup();
        let truncated = files.len() > limit;
        files.truncate(limit);
        Ok((files, truncated))
    }

    /// What the checkout ignores, relative to its root and sorted: an ignored folder once,
    /// ending in `/`, without what it holds. At most `limit`, and whether there were more.
    pub fn ignored(&self, limit: usize) -> Result<(Vec<String>, bool)> {
        let out = self.cmd(
            &[
                "ls-files",
                "-z",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
            ],
            true,
        )?;
        let mut ignored: Vec<String> = out
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect();
        ignored.sort();
        let truncated = ignored.len() > limit;
        ignored.truncate(limit);
        Ok((ignored, truncated))
    }

    /// The files whose change changes what `git status` says beyond the checkout's own files,
    /// wherever git keeps them (a linked worktree's are outside it): its index and HEAD, the
    /// branch HEAD names and the branch that one follows (a reset, a commit from elsewhere or
    /// a fetch moves them while HEAD stays), and the packed refs either may be kept in.
    pub fn status_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = vec![
            self.git_path("index")?,
            self.git_path("HEAD")?,
            self.git_path("packed-refs")?,
        ];
        if let Some(branch) = self.symbolic_head()? {
            files.push(self.git_path(&format!("refs/heads/{branch}"))?);
            let spec = format!("refs/heads/{branch}@{{upstream}}");
            let args = ["rev-parse", "--symbolic-full-name", spec.as_str()];
            let out = self.run(&args, true)?;
            if out.status.success()
                && let Ok(upstream) = parse::line(&out.stdout)
                && upstream.starts_with("refs/")
            {
                files.push(self.git_path(&upstream)?);
            }
        }
        Ok(files)
    }

    /// The files git tracks in this checkout (its index), relative to its root, unsorted.
    pub fn tracked_files(&self) -> Result<Vec<String>> {
        let out = self.cmd(&["ls-files", "-z", "--cached"], true)?;
        Ok(out
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect())
    }

    /// A repo-relative file's content as `commit` has it; `None` when the commit has no such
    /// file (deleted, never added, or a folder).
    pub fn file_at(&self, commit: &Oid, path: &str) -> Result<Option<Vec<u8>>> {
        valid_oid(commit)?;
        valid_path(path)?;
        let listed = self.cmd(&["ls-tree", "-z", &commit.0, "--", path], true)?;
        // `<mode> <type> <object>\t<path>`
        let entry = listed.split(|byte| *byte == 0).next().unwrap_or_default();
        let fields: Vec<&[u8]> = entry
            .split(|byte| *byte == b'\t')
            .next()
            .unwrap_or_default()
            .split(|byte| *byte == b' ')
            .collect();
        let [_, b"blob", object] = fields.as_slice() else {
            return Ok(None);
        };
        let object = Oid(String::from_utf8_lossy(object).into_owned());
        valid_oid(&object)?;
        self.cmd(&["cat-file", "blob", &object.0], true).map(Some)
    }

    /// Whether git tracks anything at or under a repo-relative path (a file added with
    /// `git add -f` inside an ignored folder included).
    pub fn tracks_under(&self, path: &str) -> Result<bool> {
        valid_path(path)?;
        let out = self.git.run(
            Some(&self.root),
            &["ls-files", "-z", "--cached", "--", path],
            true,
            &[("GIT_LITERAL_PATHSPECS".into(), "1".into())],
            None,
        )?;
        if !out.status.success() {
            return Err(failure(&["ls-files", "--cached", "--", path], &out));
        }
        Ok(out.stdout.iter().any(|byte| *byte != 0))
    }

    /// Whether a repo-relative file is ignored (tracked files are not ignored).
    pub fn is_ignored(&self, path: &str) -> Result<bool> {
        valid_path(path)?;
        let args = ["check-ignore", "-q", "--stdin", "-z"];
        let mut input = path.as_bytes().to_vec();
        input.push(0);
        // check-ignore consumes literal paths, not pathspecs, and rejects literal magic.
        let out = self.git.run(
            Some(&self.root),
            &args,
            true,
            &[("GIT_LITERAL_PATHSPECS".into(), "0".into())],
            Some(&input),
        )?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(failure(&args, &out)),
        }
    }

    /// A binary-capable patch between two commits or trees, without external diff programs.
    pub fn diff(&self, from: &Oid, to: &Oid) -> Result<String> {
        valid_oid(from)?;
        valid_oid(to)?;
        Ok(parse::text(&self.cmd(
            &[
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
            ],
            true,
        )?)?
        .to_owned())
    }

    /// Git's per-path numstat, including binary and renamed files.
    pub fn diff_stat(&self, from: &Oid, to: &Oid) -> Result<DiffStat> {
        valid_oid(from)?;
        valid_oid(to)?;
        parse::stat(&self.cmd(
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--numstat",
                "-z",
                "--find-renames",
                &from.0,
                &to.0,
                "--",
            ],
            true,
        )?)
    }

    /// Lossless verification input: stats plus all paths, including rename sources.
    pub fn diff_scope(&self, from: &Oid, to: &Oid) -> Result<(DiffStat, Vec<String>)> {
        valid_oid(from)?;
        valid_oid(to)?;
        parse::stat_with_paths(&self.cmd(
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--numstat",
                "-z",
                "--find-renames",
                &from.0,
                &to.0,
                "--",
            ],
            true,
        )?)
    }

    /// The best common ancestor of two commits.
    pub fn merge_base(&self, a: &Oid, b: &Oid) -> Result<Oid> {
        valid_oid(a)?;
        valid_oid(b)?;
        parse::oid(&self.cmd(&["merge-base", &a.0, &b.0], true)?)
    }

    /// Count commits reachable from to but not from from.
    pub fn count_commits(&self, from: &Oid, to: &Oid) -> Result<u32> {
        valid_oid(from)?;
        valid_oid(to)?;
        parse::line(&self.cmd(
            &[
                "rev-list",
                "--count",
                &format!("{}..{}", from.0, to.0),
                "--",
            ],
            true,
        )?)?
        .parse()
        .map_err(|_| Error::Parse("invalid commit count".into()))
    }

    pub(crate) fn tree_changes(
        &self,
        from: &Oid,
        to: &Oid,
        untracked: &BTreeSet<String>,
    ) -> Result<Vec<Change>> {
        valid_oid(from)?;
        valid_oid(to)?;
        parse::changes(
            &self.cmd(
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--name-status",
                    "-z",
                    "--find-renames",
                    &from.0,
                    &to.0,
                    "--",
                ],
                true,
            )?,
            untracked,
        )
    }
    pub(crate) fn changed_paths(&self, from: &Oid, to: &Oid) -> Result<Vec<String>> {
        let mut paths = BTreeSet::new();
        for change in self.tree_changes(from, to, &BTreeSet::new())? {
            paths.insert(change.path);
            if let ChangeKind::Renamed { from } = change.kind {
                paths.insert(from);
            }
        }
        Ok(paths.into_iter().collect())
    }

    pub(crate) fn symbolic_head(&self) -> Result<Option<String>> {
        let args = ["symbolic-ref", "-q", "HEAD"];
        let out = self.run(&args, true)?;
        match out.status.code() {
            Some(0) => Ok(Some(
                parse::line(&out.stdout)?
                    .strip_prefix("refs/heads/")
                    .ok_or_else(|| Error::Parse("HEAD is not a local branch".into()))?
                    .to_owned(),
            )),
            Some(1) => Ok(None),
            _ => Err(failure(&args, &out)),
        }
    }

    pub(crate) fn busy(&self) -> Result<Option<String>> {
        let dir = parse::path_line(&self.cmd(&["rev-parse", "--absolute-git-dir"], true)?)?;
        for name in [
            "index.lock",
            "HEAD.lock",
            "MERGE_HEAD",
            "rebase-merge",
            "rebase-apply",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "BISECT_START",
            "sequencer",
        ] {
            if dir.join(name).try_exists()? {
                return Ok(Some(name.to_owned()));
            }
        }
        Ok(None)
    }

    pub(crate) fn landing_lock(&self) -> Arc<Mutex<()>> {
        static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
        let mut locks = LOCKS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        locks.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = locks.get(&self.common_dir).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(Mutex::new(()));
        locks.insert(self.common_dir.clone(), Arc::downgrade(&lock));
        lock
    }

    /// Land only a fast-forward of the expected tip, serializing landings per common git dir.
    /// Checked-out branches use merge --ff-only --no-autostash --no-overwrite-ignore after
    /// collision/busy checks. Unchecked-out branches use compare-and-swap update-ref.
    pub fn land(&self, request: &LandRequest) -> Result<LandOutcome> {
        self.validate_branch(&request.branch)?;
        valid_oid(&request.expected_tip)?;
        valid_oid(&request.commit)?;
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let tip = || {
            self.branch_tip(&request.branch)?
                .ok_or_else(|| Error::Invalid("landing branch no longer exists".into()))
        };
        let actual = tip()?;
        if actual != request.expected_tip {
            return Ok(LandOutcome::Blocked(LandBlock::TipMoved { actual }));
        }
        if !self.ancestor(&request.expected_tip, &request.commit)? {
            return Ok(LandOutcome::Blocked(LandBlock::NotFastForward));
        }
        let holding = self
            .worktrees()?
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(&request.branch));
        if let Some(holding) = holding {
            let checkout = Repo {
                root: holding.path.clone(),
                ..self.clone()
            };
            let busy = |what| {
                LandOutcome::Blocked(LandBlock::CheckoutBusy {
                    worktree: holding.path.clone(),
                    what,
                })
            };
            if let Some(what) = checkout.busy()? {
                return Ok(busy(what));
            }
            let changed = self.changed_paths(&request.expected_tip, &request.commit)?;
            let status = checkout.status(true)?;
            let mut paths = Vec::new();
            for (path, kind) in status.entries {
                if changed.iter().any(|p| parse::overlaps(p, &path)) {
                    paths.push(CollidingPath { path, kind });
                }
            }
            paths.sort_by(|a, b| a.path.cmp(&b.path));
            paths.dedup();
            if !paths.is_empty() {
                return Ok(LandOutcome::Blocked(LandBlock::Collisions {
                    worktree: holding.path,
                    paths,
                }));
            }
            if status.unmerged {
                return Ok(busy("unmerged index entries".into()));
            }
            let now = checkout.symbolic_head()?;
            if now.as_deref() != Some(&request.branch) {
                return Ok(LandOutcome::Blocked(LandBlock::BranchSwitched {
                    worktree: holding.path,
                    now,
                }));
            }
            let actual = tip()?;
            if actual != request.expected_tip {
                return Ok(LandOutcome::Blocked(LandBlock::TipMoved { actual }));
            }
            let args = [
                "-c",
                "merge.autoStash=false",
                "merge",
                "--ff-only",
                "--no-autostash",
                "--no-overwrite-ignore",
                "--no-edit",
                &request.commit.0,
            ];
            let out = checkout.run(&args, false)?;
            if !out.status.success() {
                // A post-merge hook can exit nonzero after a successful fast-forward.
                // Never misreport an already-landed commit as a block.
                let actual = tip()?;
                if actual == request.commit {
                    return Ok(LandOutcome::Landed { new_tip: actual });
                }
                if actual != request.expected_tip {
                    return Ok(LandOutcome::Blocked(LandBlock::TipMoved { actual }));
                }
                return Ok(busy(failure(&args, &out).to_string()));
            }
            // Someone switched the checkout between the check above and the merge: the merge
            // went to the branch they switched to, and the target did not move.
            let actual = tip()?;
            if actual != request.commit {
                let now = checkout.symbolic_head()?;
                if now.as_deref() == Some(&request.branch) {
                    return Ok(LandOutcome::Blocked(LandBlock::TipMoved { actual }));
                }
                return Ok(LandOutcome::Blocked(LandBlock::BranchSwitched {
                    worktree: holding.path,
                    now,
                }));
            }
        } else {
            let actual = tip()?;
            if actual != request.expected_tip {
                return Ok(LandOutcome::Blocked(LandBlock::TipMoved { actual }));
            }
            let name = format!("refs/heads/{}", request.branch);
            let args = [
                "update-ref",
                "-m",
                "Brigadier reviewed landing",
                &name,
                &request.commit.0,
                &request.expected_tip.0,
            ];
            let out = self.run(&args, false)?;
            if !out.status.success() {
                let actual = tip()?;
                if actual != request.expected_tip {
                    return Ok(LandOutcome::Blocked(LandBlock::TipMoved { actual }));
                }
                return Ok(LandOutcome::Blocked(LandBlock::CheckoutBusy {
                    worktree: self.root.clone(),
                    what: failure(&args, &out).to_string(),
                }));
            }
        }
        Ok(LandOutcome::Landed {
            new_tip: request.commit.clone(),
        })
    }

    /// A commit on `onto` that reverts `commits` (in any order), touching no checkout or ref;
    /// `land` puts it on the branch under the landing lock. Refused when a commit after the
    /// oldest of them, other than they, changed any of their paths.
    pub fn prepare_revert(
        &self,
        commits: &[Oid],
        onto: &Oid,
        message: &str,
    ) -> Result<RevertOutcome> {
        valid_oid(onto)?;
        for commit in commits {
            valid_oid(commit)?;
            if !self.ancestor(commit, onto)? {
                return Err(Error::Invalid(format!("{} is not on the branch", commit.0)));
            }
        }
        // Newest first, so each revert applies to the tree the one before it left.
        let commits = self.newest_first(commits)?;
        let oldest = commits
            .last()
            .ok_or_else(|| Error::Invalid("no commits to revert".into()))?;
        let parent = |commit: &Oid| self.resolve(&format!("{}^", commit.0));
        let mut paths = BTreeSet::new();
        for commit in &commits {
            paths.extend(self.changed_paths(&parent(commit)?, commit)?);
        }
        let range = format!("{}..{}", oldest.0, onto.0);
        let later = self.cmd(&["rev-list", "--reverse", &range], true)?;
        let mut touched = BTreeSet::new();
        for line in String::from_utf8_lossy(&later).lines() {
            let commit = Oid(line.trim().to_owned());
            if commits.contains(&commit) {
                continue;
            }
            for path in self.changed_paths(&parent(&commit)?, &commit)? {
                if paths.iter().any(|ours| parse::overlaps(ours, &path)) {
                    touched.insert(path);
                }
            }
        }
        if !touched.is_empty() {
            return Ok(RevertOutcome::Touched {
                paths: touched.into_iter().collect(),
            });
        }
        let mut current = onto.clone();
        for commit in &commits {
            let tree = match self.replay_tree(commit, &current, &parent(commit)?)? {
                TreeMerge::Ready(tree) => tree,
                TreeMerge::Conflicts(paths) => return Ok(RevertOutcome::Conflicts { paths }),
            };
            current = self.commit_tree(&tree, &[&current], "Brigadier revert step", true)?;
        }
        let tree = parse::oid(&self.cmd(
            &["rev-parse", "--verify", &format!("{}^{{tree}}", current.0)],
            true,
        )?)?;
        let commit = self.commit_tree(&tree, &[onto], message, false)?;
        Ok(RevertOutcome::Ready { commit })
    }

    /// `commits` ordered newest first by ancestry.
    fn newest_first(&self, commits: &[Oid]) -> Result<Vec<Oid>> {
        let mut ordered: Vec<Oid> = Vec::with_capacity(commits.len());
        for commit in commits {
            valid_oid(commit)?;
            let mut at = ordered.len();
            for (index, placed) in ordered.iter().enumerate() {
                if self.ancestor(placed, commit)? {
                    at = index;
                    break;
                }
            }
            ordered.insert(at, commit.clone());
        }
        Ok(ordered)
    }

    /// Two commits whose diff is exactly what `commits` (in any order) changed: the parent of
    /// the oldest, and an unreferenced commit replaying each of them onto it, so changes
    /// that landed between them are left out. When a replay conflicts, the range from the
    /// oldest's parent to the newest instead.
    pub fn changes_of(&self, commits: &[Oid]) -> Result<(Oid, Oid)> {
        let newest_first = self.newest_first(commits)?;
        let (Some(oldest), Some(newest)) = (newest_first.last(), newest_first.first()) else {
            return Err(Error::Invalid("no commits".into()));
        };
        let from = self.resolve(&format!("{}^", oldest.0))?;
        let mut current = from.clone();
        for commit in newest_first.iter().rev() {
            let parent = self.resolve(&format!("{}^", commit.0))?;
            current = match self.replay_tree(&parent, &current, commit)? {
                TreeMerge::Ready(tree) => {
                    self.commit_tree(&tree, &[&current], "Brigadier review step", true)?
                }
                TreeMerge::Conflicts(_) => return Ok((from, newest.clone())),
            };
        }
        Ok((from, current))
    }

    /// The checkout's HEAD, index and files as trees, and its untracked files, for a review
    /// of what is uncommitted, unstaged or staged. The real index and files stay untouched.
    pub fn checkout_trees(&self) -> Result<CheckoutTrees> {
        let head = self.status(false)?.head;
        let head_tree = match &head {
            Some(head) => parse::oid(&self.cmd(
                &["rev-parse", "--verify", &format!("{}^{{tree}}", head.0)],
                true,
            )?)?,
            None => self.empty_tree()?,
        };
        let (_, files) = self.capture()?;
        let temp = TempIndex::new()?;
        let index = self.git_path("index")?;
        if index.exists() {
            copy_index(&index, &temp.path)?;
        } else {
            self.index_cmd(&temp, &["read-tree", "--empty"])?;
        }
        self.index_cmd(&temp, &["update-index", "--no-split-index"])?;
        let staged = parse::oid(&self.index_cmd(&temp, &["write-tree"])?)?;
        let listed = self.cmd(&["ls-files", "--others", "--exclude-standard", "-z"], true)?;
        let untracked = String::from_utf8_lossy(&listed)
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(CheckoutTrees {
            head,
            head_tree,
            staged,
            files,
            untracked,
        })
    }

    /// The id of the empty tree in this repository's hash.
    pub fn empty_tree(&self) -> Result<Oid> {
        parse::oid(&self.git.checked(
            Some(&self.root),
            &["hash-object", "-t", "tree", "--stdin"],
            true,
            &[],
            Some(&[]),
        )?)
    }

    /// The unified text patch between two commits or trees, as a review shows it: `context`
    /// lines around each change (a large number gives whole files), whitespace changes left
    /// out on request. Binary files are only named; text that isn't UTF-8 is shown lossily.
    pub fn review_patch(
        &self,
        from: &Oid,
        to: &Oid,
        context: u32,
        ignore_whitespace: bool,
    ) -> Result<String> {
        valid_oid(from)?;
        valid_oid(to)?;
        let context = format!("--unified={context}");
        let mut args = vec![
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--find-renames",
            &context,
        ];
        if ignore_whitespace {
            args.push("--ignore-all-space");
        }
        args.extend([from.0.as_str(), to.0.as_str(), "--"]);
        Ok(String::from_utf8_lossy(&self.cmd(&args, true)?).into_owned())
    }

    /// What changed between two commits or trees, path by path; `untracked` marks the paths
    /// git doesn't track yet.
    pub fn changes(
        &self,
        from: &Oid,
        to: &Oid,
        untracked: &BTreeSet<String>,
    ) -> Result<Vec<Change>> {
        self.tree_changes(from, to, untracked)
    }

    /// The latest `limit` commits of `tip`'s history, newest first.
    pub fn log(&self, tip: &Oid, limit: usize) -> Result<Vec<CommitInfo>> {
        valid_oid(tip)?;
        let count = format!("--max-count={limit}");
        let out = self.cmd(
            &[
                "log",
                "-z",
                "--format=%H%x1f%ct%x1f%s",
                &count,
                &tip.0,
                "--",
            ],
            true,
        )?;
        String::from_utf8_lossy(&out)
            .split('\0')
            .filter(|entry| !entry.trim().is_empty())
            .map(|entry| {
                let mut fields = entry.trim_start_matches('\n').splitn(3, '\u{1f}');
                let (Some(commit), Some(at), Some(subject)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    return Err(Error::Parse("invalid log entry".into()));
                };
                Ok(CommitInfo {
                    commit: Oid(commit.to_owned()),
                    subject: subject.to_owned(),
                    at_ms: at
                        .parse::<i64>()
                        .map_err(|_| Error::Parse("invalid commit time".into()))?
                        * 1000,
                })
            })
            .collect()
    }

    /// The repository's default branch: the one `origin/HEAD` names, else `main` or `master`.
    pub fn default_branch(&self) -> Result<Option<String>> {
        let args = ["symbolic-ref", "-q", "--short", "refs/remotes/origin/HEAD"];
        let out = self.run(&args, true)?;
        if out.status.success() {
            let remote = parse::line(&out.stdout)?;
            if let Some(name) = remote.strip_prefix("origin/")
                && self.branch_tip(name)?.is_some()
            {
                return Ok(Some(name.to_owned()));
            }
        }
        for name in ["main", "master"] {
            if self.branch_tip(name)?.is_some() {
                return Ok(Some(name.to_owned()));
            }
        }
        Ok(None)
    }

    /// The user's commit of the checkout's changes on its current branch: what is staged, or
    /// with `include_unstaged` every non-ignored change. Runs the repository's hooks as
    /// `git commit` does, under the landing lock so no landing moves the branch meanwhile.
    pub fn commit_changes(&self, message: &str, include_unstaged: bool) -> Result<Oid> {
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(what) = self.busy()? {
            return Err(Error::Invalid(format!("the checkout is busy ({what})")));
        }
        if include_unstaged {
            self.cmd(&["add", "-A", "--", "."], false)?;
        }
        self.git.checked(
            Some(&self.root),
            &["commit", "--quiet", "--file=-"],
            false,
            &[],
            Some(message.as_bytes()),
        )?;
        self.resolve("HEAD")
    }

    /// Where `branch` pushes and how many of its commits are not pushed yet.
    pub fn remote_state(&self, branch: &str) -> Result<RemoteState> {
        self.validate_branch(branch)?;
        let config = |key: String| -> Result<Option<String>> {
            let args = ["config", "--get", key.as_str()];
            let out = self.run(&args, true)?;
            Ok(match out.status.code() {
                Some(0) => Some(parse::line(&out.stdout)?),
                _ => None,
            })
        };
        let upstream_remote = config(format!("branch.{branch}.remote"))?;
        let upstream = match &upstream_remote {
            Some(_) => {
                let spec = format!("{branch}@{{upstream}}");
                // `validate_branch` keeps it from reading as an option, and rev-parse would
                // echo an `--end-of-options` back with the name.
                let args = ["rev-parse", "--abbrev-ref", spec.as_str()];
                let out = self.run(&args, true)?;
                out.status
                    .success()
                    .then(|| parse::line(&out.stdout))
                    .transpose()?
            }
            None => None,
        };
        let remote = match upstream_remote {
            Some(remote) => Some(remote),
            None => config("remote.origin.url".into())?.map(|_| "origin".to_owned()),
        };
        let tip = format!("refs/heads/{branch}");
        let ahead = match (&remote, &upstream) {
            (_, Some(upstream)) => parse::line(&self.cmd(
                &["rev-list", "--count", &format!("{upstream}..{tip}"), "--"],
                true,
            )?)?,
            (Some(_), None) => parse::line(&self.cmd(
                &["rev-list", "--count", &tip, "--not", "--remotes", "--"],
                true,
            )?)?,
            (None, None) => "0".to_owned(),
        };
        Ok(RemoteState {
            remote,
            upstream,
            ahead: ahead
                .parse()
                .map_err(|_| Error::Parse("invalid commit count".into()))?,
        })
    }

    /// Pushes `branch` to its upstream, or to `origin` as its new upstream. Never forces, and
    /// git asks nothing on a terminal.
    pub fn push(&self, branch: &str) -> Result<()> {
        let state = self.remote_state(branch)?;
        let remote = state
            .remote
            .ok_or_else(|| Error::Invalid("the repository has no remote to push to".into()))?;
        let refspec = format!("refs/heads/{branch}");
        let mut args = vec!["push", "--porcelain"];
        if state.upstream.is_none() {
            args.push("--set-upstream");
        }
        args.extend([remote.as_str(), refspec.as_str()]);
        let env: Environment = vec![("GIT_TERMINAL_PROMPT".into(), "0".into())];
        self.git
            .checked(Some(&self.root), &args, false, &env, None)?;
        Ok(())
    }

    pub(crate) fn merge_tree(&self, left: &Oid, right: &Oid) -> Result<TreeMerge> {
        Ok(match self.marked_merge(left, right)? {
            (tree, None) => TreeMerge::Ready(tree),
            (_, Some(paths)) => TreeMerge::Conflicts(paths),
        })
    }

    /// Merges two commits without touching any checkout: the merged tree, with conflict markers
    /// left in conflicted files, and the conflicted paths (`None` when the merge is clean).
    fn marked_merge(&self, left: &Oid, right: &Oid) -> Result<(Oid, Option<Vec<String>>)> {
        valid_oid(left)?;
        valid_oid(right)?;
        let args = [
            "merge-tree",
            "--write-tree",
            "--name-only",
            "-z",
            &left.0,
            &right.0,
        ];
        let out = self.run(&args, false)?;
        if !matches!(out.status.code(), Some(0 | 1)) {
            return Err(failure(&args, &out));
        }
        let mut fields = parse::fields(&out.stdout);
        let tree = parse::oid(
            fields
                .next()
                .ok_or_else(|| Error::Parse("merge-tree omitted tree".into()))?,
        )?;
        if out.status.success() {
            Ok((tree, None))
        } else {
            let paths = fields
                .take_while(|s| !s.is_empty())
                .map(|s| parse::text(s).map(str::to_owned))
                .collect::<Result<BTreeSet<_>>>()?;
            Ok((tree, Some(paths.into_iter().collect())))
        }
    }

    /// `work`'s commits since `base` rebuilt with `leave_out` as it was at `base` in each
    /// (restored where `base` had a file there, removed otherwise), their messages, authors and
    /// merged parents kept and a commit left with no change dropped. A commit before the first
    /// one that changes stays itself, so the merge base does too. Returns the new tip (`work`
    /// itself when nothing is left out) and each rewritten commit with what replaced it. A
    /// merge task starts from it, so a conflicting task's stray files reach neither the merge
    /// nor the history it lands.
    pub fn work_without(
        &self,
        base: &Oid,
        work: &Oid,
        leave_out: &[String],
    ) -> Result<(Oid, Vec<(Oid, Oid)>)> {
        valid_oid(base)?;
        valid_oid(work)?;
        if leave_out.is_empty() {
            return Ok((work.clone(), Vec::new()));
        }
        let range = format!("{}..{}", base.0, work.0);
        let list = self.cmd(
            &[
                "rev-list",
                "--reverse",
                "--first-parent",
                "--parents",
                &range,
            ],
            true,
        )?;
        let mut tip: Option<Oid> = None;
        let mut rebuilt = Vec::new();
        for line in parse::text(&list)?.lines() {
            let mut ids = line.split_whitespace().map(|id| Oid(id.to_owned()));
            let (Some(commit), Some(parent)) = (ids.next(), ids.next()) else {
                return Err(Error::Invalid(
                    "the work holds a commit without a parent".into(),
                ));
            };
            // A merge's other parents (the target it merged, say) stay, so its ancestry does.
            let merged: Vec<Oid> = ids.collect();
            let onto = tip.get_or_insert(parent.clone()).clone();
            let tree = self.tree_reverting(&commit, base, leave_out)?;
            if onto == parent && tree == self.tree_of(&commit.0)? {
                // Nothing rewritten below it and nothing to leave out: the commit itself.
                tip = Some(commit);
                continue;
            }
            if merged.is_empty() && tree == self.tree_of(&onto.0)? {
                rebuilt.push((commit, onto));
                continue;
            }
            let parents: Vec<&Oid> = std::iter::once(&onto).chain(&merged).collect();
            let (message, author) = self.authored(&commit)?;
            let new = self.commit_tree_with(&tree, &parents, &message, &author)?;
            rebuilt.push((commit, new.clone()));
            tip = Some(new);
        }
        Ok((tip.unwrap_or_else(|| work.clone()), rebuilt))
    }

    /// A commit's exact message and its author's name, email and date as git environment.
    pub(crate) fn authored(&self, commit: &Oid) -> Result<(String, Vec<(OsString, OsString)>)> {
        let raw = self.cmd(&["cat-file", "commit", &commit.0], true)?;
        let raw = parse::text(&raw)?;
        let (headers, message) = raw
            .split_once("\n\n")
            .unwrap_or((raw.strip_suffix('\n').unwrap_or(raw), ""));
        let author = headers
            .lines()
            .find_map(|l| l.strip_prefix("author "))
            .ok_or_else(|| Error::Parse("commit without an author".into()))?;
        let (ident, date) = author
            .rsplit_once('>')
            .ok_or_else(|| Error::Parse("malformed commit author".into()))?;
        let (name, email) = ident
            .split_once('<')
            .ok_or_else(|| Error::Parse("malformed commit author".into()))?;
        let env = [
            ("GIT_AUTHOR_NAME", name.trim_end()),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_AUTHOR_DATE", date.trim()),
        ]
        .map(|(key, value)| (key.into(), value.into()))
        .into();
        Ok((message.to_owned(), env))
    }

    /// Every file path in `commit`'s tree.
    pub fn paths_at(&self, commit: &Oid) -> Result<BTreeSet<String>> {
        valid_oid(commit)?;
        let listed = self.cmd(&["ls-tree", "-r", "-z", "--name-only", &commit.0], true)?;
        Ok(listed
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect())
    }

    /// A merge worker's result `tip` checked against the two sides it merged, `ours` and
    /// `theirs` (SESSION-FIXES-PLAN.md Q2): whether both are merged into it, what of either
    /// side's changes vanished from its tree, and the files the merge brought in or changed
    /// (to look for conflict markers).
    pub fn audit_merge(&self, tip: &Oid, ours: &Oid, theirs: &Oid) -> Result<MergeAudit> {
        valid_oid(tip)?;
        let merged = self.ancestor(ours, tip)?
            && self.ancestor(theirs, tip)?
            && (self.ancestor(ours, theirs)?
                || self.ancestor(theirs, ours)?
                || self.merge_joining(tip, ours, theirs)?);
        let base = self.merge_base(ours, theirs)?;
        let none = BTreeSet::new();
        let ours_changes = self.tree_changes(&base, ours, &none)?;
        let theirs_changes = self.tree_changes(&base, theirs, &none)?;
        let present = self.paths_at(tip)?;
        let missing = vanished(&ours_changes, &theirs_changes, |path| {
            present.contains(path)
        });
        // What the merge itself wrote: a file as neither side has it. One taken whole from a
        // side is as that side made it.
        let differs = |side: &Oid| -> Result<BTreeSet<String>> {
            Ok(self
                .tree_changes(side, tip, &none)?
                .into_iter()
                .filter(|change| change.kind != ChangeKind::Deleted)
                .map(|change| change.path)
                .collect())
        };
        let from_theirs = differs(theirs)?;
        let touched = differs(ours)?
            .into_iter()
            .filter(|path| from_theirs.contains(path))
            .collect();
        Ok(MergeAudit {
            merged,
            missing,
            touched,
        })
    }

    /// Whether `tip`'s history holds a merge commit with one parent at or after `ours` and
    /// another at or after `theirs`.
    fn merge_joining(&self, tip: &Oid, ours: &Oid, theirs: &Oid) -> Result<bool> {
        valid_oid(ours)?;
        valid_oid(theirs)?;
        let listed = self.cmd(
            &[
                "rev-list",
                "--merges",
                "--parents",
                &tip.0,
                &format!("^{}", ours.0),
                &format!("^{}", theirs.0),
            ],
            true,
        )?;
        for line in String::from_utf8_lossy(&listed).lines() {
            let parents: Vec<Oid> = line
                .split_whitespace()
                .skip(1)
                .map(|p| Oid(p.to_owned()))
                .collect();
            for (i, one) in parents.iter().enumerate() {
                if !self.ancestor(ours, one)? {
                    continue;
                }
                for (j, other) in parents.iter().enumerate() {
                    if i != j && self.ancestor(theirs, other)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    /// `commit`'s tree with `paths` as they were at `base`: restored where `base` had a file
    /// there, removed otherwise.
    pub(crate) fn tree_reverting(&self, commit: &Oid, base: &Oid, paths: &[String]) -> Result<Oid> {
        for path in paths {
            valid_path(path)?;
        }
        let mut ls = vec!["ls-tree", "-z", "--full-tree", &base.0, "--"];
        ls.extend(paths.iter().map(String::as_str));
        // Only what sat at exactly these paths: a folder that a path now replaces keeps its
        // files' own changes (each has its own verdict), so its tree entry is not restored.
        let at_base: Vec<u8> = self
            .cmd(&ls, true)?
            .split(|&c| c == 0)
            .filter(|record| {
                let header = record.split(|&c| c == b'\t').next().unwrap_or_default();
                !record.is_empty() && header.split(|&c| c == b' ').nth(1) != Some(b"tree")
            })
            .flat_map(|record| record.iter().copied().chain([0]))
            .collect();
        let index = TempIndex::new()?;
        self.index_cmd(&index, &["read-tree", &commit.0])?;
        let mut remove = vec!["update-index", "--force-remove", "--"];
        remove.extend(paths.iter().map(String::as_str));
        self.index_cmd(&index, &remove)?;
        if !at_base.is_empty() {
            // ls-tree's "<mode> <type> <oid>\t<path>" records are what --index-info reads.
            self.git.checked(
                Some(&self.root),
                &[
                    "-c",
                    "core.splitIndex=false",
                    "update-index",
                    "-z",
                    "--index-info",
                ],
                false,
                &index.env(),
                Some(&at_base),
            )?;
        }
        parse::oid(&self.index_cmd(&index, &["write-tree"])?)
    }

    pub(crate) fn replay_tree(&self, base: &Oid, onto: &Oid, tree: &Oid) -> Result<TreeMerge> {
        Ok(match self.marked_replay(base, onto, tree)? {
            (tree, None) => TreeMerge::Ready(tree),
            (_, Some(paths)) => TreeMerge::Conflicts(paths),
        })
    }

    /// [`Repo::replay_tree`] keeping the merged tree when it conflicts, markers in place:
    /// `onto`'s side first, `tree`'s second.
    fn marked_replay(
        &self,
        base: &Oid,
        onto: &Oid,
        tree: &Oid,
    ) -> Result<(Oid, Option<Vec<String>>)> {
        // Explicit merge-base was added after 2.38. Give two synthetic commits the exact
        // common parent instead, so snapshot-only content is excluded from the worker delta.
        let onto_tree = parse::oid(&self.cmd(
            &["rev-parse", "--verify", &format!("{}^{{tree}}", onto.0)],
            true,
        )?)?;
        let left = self.commit_tree(&onto_tree, &[base], "Brigadier replay target", true)?;
        let source_tree = parse::oid(&self.cmd(
            &["rev-parse", "--verify", &format!("{}^{{tree}}", tree.0)],
            true,
        )?)?;
        let right = self.commit_tree(&source_tree, &[base], "Brigadier replay source", true)?;
        self.marked_merge(&left, &right)
    }

    /// Prepare a fast-forward or a two-parent session merge without touching any checkout/ref.
    /// A separate land call performs the guarded update after approval.
    pub fn prepare_merge(&self, base: &str, branch: &str, message: &str) -> Result<MergeOutcome> {
        let tip = self
            .branch_tip(branch)?
            .ok_or_else(|| Error::Invalid("session branch does not exist".into()))?;
        self.prepare_merge_commit(base, &tip, message)
    }

    /// Prepare only the approved commit, leaving later branch work out of the merge.
    pub fn prepare_merge_commit(
        &self,
        base: &str,
        tip: &Oid,
        message: &str,
    ) -> Result<MergeOutcome> {
        let base_tip = self
            .branch_tip(base)?
            .ok_or_else(|| Error::Invalid("base branch does not exist".into()))?;
        let tip = tip.clone();
        if self.ancestor(&base_tip, &tip)? {
            return Ok(MergeOutcome::Ready {
                commit: tip,
                base_tip,
                fast_forward: true,
            });
        }
        if self.ancestor(&tip, &base_tip)? {
            return Ok(MergeOutcome::Ready {
                commit: base_tip.clone(),
                base_tip,
                fast_forward: true,
            });
        }
        match self.merge_tree(&base_tip, &tip)? {
            TreeMerge::Conflicts(paths) => Ok(MergeOutcome::Conflicts { paths }),
            TreeMerge::Ready(tree) => {
                let commit = self.commit_tree(&tree, &[&base_tip, &tip], message, false)?;
                Ok(MergeOutcome::Ready {
                    commit,
                    base_tip,
                    fast_forward: false,
                })
            }
        }
    }
}

/// Whether `path` holds the letters of `query` in order, ignoring case (as the UI's fuzzy
/// match finds a file).
fn has_letters(path: &str, query: &str) -> bool {
    let mut letters = path.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|want| letters.any(|letter| letter == want))
}

fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if path
        .components()
        .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err(Error::Invalid("worktree path must not contain ..".into()));
    }
    Ok(path)
}

/// The author, committer and date of a commit Brigadier makes for itself and no ref names, so
/// the same files on the same parent always give the same commit.
/// A private copy of the real index `from`, with its time. Git trusts an entry's cached stat
/// only when the entry is older than the index file; a copy timed now would pass a file
/// rewritten to the same size in the index's own second (git compares whole seconds on Linux)
/// as unchanged, its old content.
fn copy_index(from: &Path, to: &Path) -> Result<()> {
    fs::copy(from, to)?;
    let modified = fs::metadata(from)?.modified()?;
    fs::File::options()
        .write(true)
        .open(to)?
        .set_modified(modified)?;
    Ok(())
}

fn fixed_identity() -> Environment {
    let date: std::ffi::OsString = "2000-01-01T00:00:00+0000".into();
    ["GIT_AUTHOR_NAME", "GIT_COMMITTER_NAME"]
        .map(|key| (key.into(), "Brigadier".into()))
        .into_iter()
        .chain(
            ["GIT_AUTHOR_EMAIL", "GIT_COMMITTER_EMAIL"]
                .map(|key| (key.into(), "brigadier@localhost".into())),
        )
        .chain(["GIT_AUTHOR_DATE", "GIT_COMMITTER_DATE"].map(|key| (key.into(), date.clone())))
        .collect()
}

/// What of either side's changes since their merge base is missing from a merge's tree
/// (`present`): a path one side added or changed and the other left alone must be there; one
/// side changed and the other deleted is contested, and may go only with a reason; a path one
/// side deleted (or renamed away) while the other left it alone may simply be gone.
pub(crate) fn vanished(
    ours: &[Change],
    theirs: &[Change],
    present: impl Fn(&str) -> bool,
) -> Vec<Vanished> {
    use std::collections::BTreeMap;
    // Per side: the paths it now has (added, changed, renamed to) and those it removed.
    let sides = |changes: &[Change]| {
        let (mut has, mut deleted, mut renamed) =
            (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
        for change in changes {
            match &change.kind {
                ChangeKind::Deleted => {
                    deleted.insert(change.path.clone());
                }
                ChangeKind::Renamed { from } => {
                    renamed.insert(from.clone());
                    has.insert(change.path.clone());
                }
                _ => {
                    has.insert(change.path.clone());
                }
            }
        }
        (has, deleted, renamed)
    };
    let (ours_has, ours_deleted, ours_renamed) = sides(ours);
    let (theirs_has, theirs_deleted, theirs_renamed) = sides(theirs);
    let mut gone = BTreeMap::new();
    for (has, side, other_deleted, other_renamed) in [
        (&ours_has, Side::Ours, &theirs_deleted, &theirs_renamed),
        (&theirs_has, Side::Theirs, &ours_deleted, &ours_renamed),
    ] {
        for path in has {
            // Renamed away by the other side: its changes went with the new name.
            if present(path) || other_renamed.contains(path) {
                continue;
            }
            gone.entry(path.clone()).or_insert(Vanished {
                path: path.clone(),
                side,
                contested: other_deleted.contains(path),
            });
        }
    }
    gone.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::vanished;
    use crate::{Git, Oid};
    use std::{ffi::OsString, fs, path::PathBuf};

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = std::path::Path;

        fn deref(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Git in a fresh temp folder, kept from the user's own config; the folder too.
    fn test_git(name: &str) -> (Temp, Git) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "brigadier-git-{name}-{}-{nanos}",
            std::process::id()
        ));
        let dir = Temp(dir);
        fs::create_dir_all(&*dir).expect("a temp folder");
        // The user's own git config (signing, hooks, identity) stays out of it.
        let config = dir.join("gitconfig");
        fs::write(&config, "").expect("an empty config");
        let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
            .collect();
        env.push(("GIT_CONFIG_GLOBAL".into(), config.into_os_string()));
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ] {
            env.push((key.into(), value.into()));
        }
        (dir, Git::new(PathBuf::from("git"), env))
    }

    #[test]
    fn commits_holding_the_same_files_have_the_same_tree() {
        let (dir, git) = test_git("tree");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "a\n").expect("a file");
        let first = repo.commit_changes("Add a", true).expect("a commit");
        repo.cmd(
            &["commit", "--quiet", "--allow-empty", "-m", "Again"],
            false,
        )
        .expect("an empty commit");
        let again = repo.resolve("HEAD").expect("HEAD");
        fs::write(root.join("a.txt"), "b\n").expect("a change");
        let changed = repo.commit_changes("Change a", true).expect("a commit");
        let tree = |commit: &Oid| repo.tree_of(&commit.0).expect("its tree");
        assert_ne!(first, again);
        assert_eq!(tree(&first), tree(&again));
        assert_ne!(tree(&first), tree(&changed));
        // A tree is not a commit.
        assert!(repo.tree_of(&tree(&first).0).is_err());
    }

    #[test]
    fn a_later_snapshot_on_the_earlier_one_shows_only_what_changed_between_them() {
        let (dir, git) = test_git("on-base");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "a\n").unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();
        let head = repo.commit_changes("Base", true).unwrap();
        // The checkout was dirty before: a.txt changed. Then b.txt changes, and a.txt goes
        // back to HEAD's.
        fs::write(root.join("a.txt"), "user\n").unwrap();
        let before = repo.snapshot_files().unwrap();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        fs::write(root.join("b.txt"), "b2\n").unwrap();
        let after = repo.snapshot_files().unwrap();
        assert!(!repo.ancestor(&before, &after).unwrap());
        let linked = repo.on_base(&before, &after).unwrap();
        assert!(repo.ancestor(&before, &linked).unwrap());
        assert_eq!(
            repo.tree_of(&linked.0).unwrap(),
            repo.tree_of(&after.0).unwrap()
        );
        let diff = |range: &str| {
            String::from_utf8_lossy(&repo.cmd(&["diff", "--name-only", range], true).unwrap())
                .into_owned()
        };
        // From the merge base: the sibling shows the user's old edit as gone and misses
        // nothing else; the linked commit shows both real changes and nothing more.
        assert_eq!(diff(&format!("{}...{}", before.0, after.0)), "b.txt\n");
        assert_eq!(
            diff(&format!("{}...{}", before.0, linked.0)),
            "a.txt\nb.txt\n"
        );
        // The same pair gives the same commit; an ancestor base keeps the tip itself.
        assert_eq!(repo.on_base(&before, &after).unwrap(), linked);
        assert_eq!(repo.on_base(&head, &after).unwrap(), after);
        assert_eq!(repo.resolve("HEAD").unwrap(), head);
    }

    /// A file rewritten to the same size within the second the index was written in reads as
    /// changed: the snapshot's private copy of the index keeps the real one's time, so git
    /// checks such a racily clean entry's content instead of trusting its stat.
    #[test]
    fn a_snapshot_reads_a_same_size_rewrite_in_the_index_s_own_second() {
        let (dir, git) = test_git("racy");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        // Only the size and the whole second of the times count, as on a git built without
        // nanoseconds (Linux distributions' own).
        repo.cmd(&["config", "core.checkStat", "minimal"], false)
            .unwrap();
        repo.cmd(&["config", "core.trustctime", "false"], false)
            .unwrap();
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let touch = |path: &std::path::Path| {
            fs::File::options()
                .write(true)
                .open(path)
                .and_then(|file| file.set_modified(at))
                .unwrap();
        };
        let file = root.join("config.txt");
        fs::write(&file, "mode = a\n").unwrap();
        touch(&file);
        let head = repo.commit_changes("Base", true).unwrap();
        touch(&repo.git_path("index").unwrap());
        fs::write(&file, "mode = b\n").unwrap();
        touch(&file);
        let snapshot = repo.snapshot_files().unwrap();
        assert_ne!(snapshot, head, "the rewrite was missed");
        let shown = repo
            .cmd(&["show", &format!("{}:config.txt", snapshot.0)], true)
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&shown), "mode = b\n");
    }

    #[test]
    fn work_without_its_litter_keeps_it_out_of_every_commit() {
        let (dir, git) = test_git("without");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "a\n").unwrap();
        let base = repo.commit_changes("Base", true).unwrap();
        // The task commits a clean step, then a log with its next one, then a step of its own.
        fs::write(root.join("a.txt"), "a2\n").unwrap();
        let clean = repo.commit_changes("Change a", true).unwrap();
        fs::write(root.join("debug.log"), "scratch\n").unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();
        let with_log = repo.commit_changes("Add b", true).unwrap();
        fs::write(root.join("c.txt"), "c\n").unwrap();
        let work = repo.commit_changes("Add c", true).unwrap();
        let leave_out = vec!["debug.log".to_owned()];

        // A rewrite a second later has another committer date, so it would show.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let (without, rebuilt) = repo.work_without(&base, &work, &leave_out).unwrap();
        // The clean step stays itself, so a merge base on it does; the two after it are
        // rewritten, each named with what replaced it.
        assert!(repo.ancestor(&clean, &without).unwrap());
        assert_eq!(
            rebuilt.iter().map(|(from, _)| from).collect::<Vec<_>>(),
            [&with_log, &work]
        );
        assert_eq!(rebuilt[1].1, without);
        let paths = repo.paths_at(&without).unwrap();
        assert!(
            paths.contains("b.txt") && paths.contains("c.txt"),
            "{paths:?}"
        );
        assert!(!paths.contains("debug.log"), "{paths:?}");
        // No commit it carries holds the log, and each keeps its message.
        let history = repo
            .cmd(
                &["log", "--format=%s", &format!("{}..{}", base.0, without.0)],
                true,
            )
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&history),
            "Add c\nAdd b\nChange a\n"
        );
        let touched = repo
            .cmd(
                &[
                    "log",
                    "--format=%H",
                    &format!("{}..{}", base.0, without.0),
                    "--",
                    "debug.log",
                ],
                true,
            )
            .unwrap();
        assert!(touched.is_empty(), "{}", String::from_utf8_lossy(&touched));
        assert!(repo.ancestor(&base, &without).unwrap());

        // A task that merged its target keeps the target in its ancestry.
        let run = |args: &[&str]| {
            repo.cmd(args, false).expect("git");
        };
        run(&["checkout", "-q", "-b", "target", &base.0]);
        fs::write(root.join("t.txt"), "t\n").unwrap();
        let target = repo.commit_changes("Target", true).unwrap();
        run(&["checkout", "-q", "-B", "task", &work.0]);
        run(&["merge", "-q", "--no-edit", "target"]);
        let merged = repo.resolve("HEAD").unwrap();
        let (without, rebuilt) = repo.work_without(&base, &merged, &leave_out).unwrap();
        assert!(repo.ancestor(&target, &without).unwrap());
        assert!(!repo.paths_at(&without).unwrap().contains("debug.log"));
        assert_eq!(rebuilt.last(), Some(&(merged, without)));
        // Nothing to leave out: the work itself.
        assert_eq!(
            repo.work_without(&base, &work, &[]).unwrap(),
            (work, Vec::new())
        );
    }

    #[test]
    fn a_merge_is_audited_for_its_inputs_and_what_vanished() {
        use crate::{Change, ChangeKind, Side, Vanished};
        let (dir, git) = test_git("audit");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let run = |args: &[&str]| {
            repo.cmd(args, false).expect("git");
        };
        for file in ["shared.txt", "c.txt", "d.txt"] {
            fs::write(root.join(file), "base\n").unwrap();
        }
        repo.commit_changes("Base", true).unwrap();
        run(&["branch", "-M", "main"]);
        run(&["checkout", "-q", "-b", "session"]);
        fs::write(root.join("s.txt"), "s\n").unwrap();
        fs::remove_file(root.join("c.txt")).unwrap();
        fs::write(root.join("shared.txt"), "session\n").unwrap();
        let ours = repo.commit_changes("Session", true).unwrap();
        run(&["checkout", "-q", "main"]);
        fs::write(root.join("m.txt"), "m\n").unwrap();
        fs::write(root.join("c.txt"), "main changed\n").unwrap();
        fs::remove_file(root.join("d.txt")).unwrap();
        let theirs = repo.commit_changes("Main", true).unwrap();
        run(&["checkout", "-q", "session"]);
        // Not merged yet: nothing joins them.
        let before = repo.audit_merge(&ours, &ours, &theirs).unwrap();
        assert!(!before.merged);
        // The merge: shared.txt conflicts (kept as the session's), c.txt is modify/delete
        // (dropped), m.txt is left out by mistake; d.txt goes as main deleted it.
        let _ = repo.run(&["merge", "-q", "--no-edit", "main"], false);
        fs::write(root.join("shared.txt"), "both\n").unwrap();
        run(&["rm", "-q", "--cached", "--ignore-unmatch", "c.txt"]);
        let _ = fs::remove_file(root.join("c.txt"));
        run(&["rm", "-q", "-f", "m.txt"]);
        run(&["add", "-A"]);
        run(&["commit", "-q", "--no-edit"]);
        let merge = repo.resolve("HEAD").unwrap();
        let audit = repo.audit_merge(&merge, &ours, &theirs).unwrap();
        assert!(audit.merged);
        assert_eq!(
            audit.missing,
            vec![
                Vanished {
                    path: "c.txt".into(),
                    side: Side::Theirs,
                    contested: true
                },
                Vanished {
                    path: "m.txt".into(),
                    side: Side::Theirs,
                    contested: false
                },
            ]
        );
        assert!(audit.touched.contains(&"shared.txt".to_owned()));
        // A correction commit after the merge: still merged, m.txt back.
        fs::write(root.join("m.txt"), "m\n").unwrap();
        let fixed = repo.commit_changes("Restore m.txt", true).unwrap();
        let audit = repo.audit_merge(&fixed, &ours, &theirs).unwrap();
        assert!(audit.merged);
        assert_eq!(audit.missing.len(), 1, "{:?}", audit.missing);
        // Only files neither side had are the merge's own: m.txt came whole from main.
        assert_eq!(audit.touched, ["shared.txt"]);
        // One side renamed a file the other changed: the change went with the new name.
        let renamed_away = vanished(
            &[Change {
                path: "new.rs".into(),
                kind: ChangeKind::Renamed {
                    from: "old.rs".into(),
                },
                untracked: false,
            }],
            &[Change {
                path: "old.rs".into(),
                kind: ChangeKind::Modified,
                untracked: false,
            }],
            |path| path == "new.rs",
        );
        assert_eq!(
            renamed_away,
            Vec::new(),
            "a rename carries the other side's change"
        );
    }

    #[test]
    fn a_trailer_is_found_only_in_its_range() {
        let (dir, git) = test_git("trailer");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "a\n").expect("a file");
        let start = repo.commit_changes("Add a", true).expect("a commit");
        fs::write(root.join("a.txt"), "b\n").expect("a change");
        let plain = repo
            .commit_changes(
                "Change a\n\nSays Brigadier-Author: thread in its body.",
                true,
            )
            .expect("a commit");
        assert!(
            !repo
                .has_trailer(&start, &plain, "Brigadier-Author", "thread")
                .unwrap()
        );
        fs::write(root.join("a.txt"), "c\n").expect("a change");
        let marked = repo
            .commit_changes("Change a again\n\nBrigadier-Author: thread", true)
            .expect("a commit");
        assert!(
            repo.has_trailer(&start, &marked, "Brigadier-Author", "thread")
                .unwrap()
        );
        assert!(
            repo.has_trailer(&plain, &marked, "brigadier-author", "Thread")
                .unwrap()
        );
        assert!(
            !repo
                .has_trailer(&marked, &marked, "Brigadier-Author", "thread")
                .unwrap()
        );
    }

    #[test]
    fn a_trailers_values_are_listed_per_commit() {
        let (dir, git) = test_git("trailer-values");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "a\n").expect("a file");
        let start = repo.commit_changes("Add a", true).expect("a commit");
        fs::write(root.join("a.txt"), "b\n").expect("a change");
        repo.commit_changes(
            "Change a\n\nBrigadier-Author: thread\nBrigadier-Fixes: R2.2, R2.3",
            true,
        )
        .expect("a commit");
        fs::write(root.join("a.txt"), "c\n").expect("a change");
        let tip = repo
            .commit_changes("Change a again\n\nBrigadier-Author: thread", true)
            .expect("a commit");
        assert_eq!(
            repo.trailer_values(&start, &tip, "Brigadier-Fixes")
                .unwrap(),
            vec![Vec::<String>::new(), vec!["R2.2, R2.3".to_owned()]]
        );
        assert!(
            repo.trailer_values(&tip, &tip, "Brigadier-Fixes")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn the_lines_of_the_commits_with_a_trailer_are_counted() {
        let (dir, git) = test_git("trailer-stat");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "1\n2\n3\n").expect("a file");
        let start = repo.commit_changes("Add a", true).expect("a commit");
        // Marked: two lines added in a, one changed (one removed, one added), a new file of 2.
        fs::write(root.join("a.txt"), "1\ntwo\n3\n4\n5\n").expect("a change");
        fs::write(root.join("b.txt"), "b\nb\n").expect("a file");
        repo.commit_changes(
            "Edit a and add b\n\nCo-Authored-By: Someone <s@example.com>\nBrigadier-Author: thread",
            true,
        )
        .expect("a commit");
        // Not marked (the user's own, or a worker's): left out.
        fs::write(root.join("c.txt"), "c\nc\nc\n").expect("a file");
        repo.commit_changes("Add c\n\nSays Brigadier-Author: thread in its body.", true)
            .expect("a commit");
        // Marked again: one line removed, and a binary file, which counts no lines.
        fs::write(root.join("a.txt"), "1\ntwo\n3\n4\n").expect("a change");
        fs::write(root.join("d.bin"), [0u8, 1, 2, 0, 255]).expect("a binary file");
        let tip = repo
            .commit_changes("Trim a\n\nBrigadier-Author: Thread", true)
            .expect("a commit");
        let stat = repo
            .trailer_stat(&start, &tip, "Brigadier-Author", "thread", &[])
            .unwrap();
        assert_eq!(
            stat,
            crate::TrailerStat {
                commits: 2,
                added: 5,
                removed: 2,
            }
        );
        assert_eq!(
            repo.trailer_stat(&tip, &tip, "Brigadier-Author", "thread", &[])
                .unwrap(),
            crate::TrailerStat::default()
        );
        // A commit known to be the thread's without the trailer (the user's own checkout)
        // counts too.
        let unmarked = repo.resolve(&format!("{}~1", tip.0)).unwrap();
        let before = repo.resolve(&format!("{}~2", tip.0)).unwrap();
        assert_eq!(
            repo.trailer_stat(
                &start,
                &tip,
                "Brigadier-Author",
                "thread",
                &[(before, unmarked)]
            )
            .unwrap(),
            crate::TrailerStat {
                commits: 3,
                added: 8,
                removed: 2,
            }
        );
    }
}
