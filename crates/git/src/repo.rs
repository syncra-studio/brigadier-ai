use crate::{
    Branch, Change, ChangeKind, CheckoutTrees, CollidingPath, CommitInfo, DiffStat, Environment,
    Error, Git, LandBlock, LandOutcome, LandRequest, MergeOutcome, MergeStart, Oid, PatchOutcome,
    RemoteState, RepoState, Result, RevertOutcome, Snapshot, Worktree, WorktreeInfo, WorktreeSpec,
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

    pub(crate) fn run<S: AsRef<OsStr>>(&self, args: &[S], read_only: bool) -> Result<Output> {
        self.git.run(Some(&self.root), args, read_only, &[], None)
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
            fs::copy(index, &temp.path)?;
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
            fs::copy(index, &temp.path)?;
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

    /// Brings `branch` into a task's work for a merge worker to finish, without touching any
    /// checkout or ref. The work's changes since `base`, with `leave_out` kept as it was at
    /// `base`, are replayed onto
    /// the branch tip as landing would, conflict markers left in the files (the branch's side
    /// first, the work's second). The result is committed with parents the work (a commit on
    /// `work` with `leave_out` reverted) and the branch tip; a worktree started there lets the worker
    /// resolve the conflicts by editing files alone, without git.
    pub fn merge_for_resolution(
        &self,
        base: &Oid,
        work: &Oid,
        leave_out: &[String],
        branch: &str,
        message: &str,
    ) -> Result<MergeStart> {
        valid_oid(base)?;
        valid_oid(work)?;
        let onto = self
            .branch_tip(branch)?
            .ok_or_else(|| Error::Invalid(format!("branch {branch} does not exist")))?;
        let work = if leave_out.is_empty() {
            work.clone()
        } else {
            let tree = self.tree_reverting(work, base, leave_out)?;
            self.commit_tree(&tree, &[work], "Work to merge, stray files left out", false)?
        };
        let (tree, conflicts) = self.marked_replay(base, &onto, &work)?;
        let commit = self.commit_tree(&tree, &[&work, &onto], message, false)?;
        Ok(MergeStart {
            commit,
            work,
            onto,
            clean: conflicts.is_none(),
            conflicts: conflicts.unwrap_or_default(),
        })
    }

    /// The merge [`Repo::merge_for_resolution`] made as `commit` from work based on `base`,
    /// read back from the commit's two parents; `None` when it has not exactly two.
    pub fn merge_at(&self, commit: &Oid, base: &Oid) -> Result<Option<MergeStart>> {
        valid_oid(commit)?;
        valid_oid(base)?;
        let line = self.cmd(&["rev-list", "--parents", "-n", "1", &commit.0], true)?;
        let ids: Vec<Oid> = parse::line(&line)?
            .split_whitespace()
            .map(|id| Oid(id.to_owned()))
            .collect();
        let [_, work, onto] = ids.as_slice() else {
            return Ok(None);
        };
        let (_, conflicts) = self.marked_replay(base, onto, work)?;
        Ok(Some(MergeStart {
            commit: commit.clone(),
            work: work.clone(),
            onto: onto.clone(),
            clean: conflicts.is_none(),
            conflicts: conflicts.unwrap_or_default(),
        }))
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

#[cfg(test)]
mod tests {
    use crate::{Git, Oid};
    use std::{ffi::OsString, fs, path::PathBuf};

    #[test]
    fn commits_holding_the_same_files_have_the_same_tree() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("brigadier-git-tree-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("a temp folder");
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
        let git = Git::new(PathBuf::from("git"), env);
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
        let _ = fs::remove_dir_all(&dir);
    }
}
