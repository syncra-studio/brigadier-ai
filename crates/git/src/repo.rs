use crate::{
    Branch, Change, ChangeKind, CheckoutTrees, CollidingPath, CommitInfo, DiffStat, Environment,
    Error, Git, LandBlock, LandOutcome, LandRequest, MergeAudit, MergeOutcome, Oid, PatchOutcome,
    RemoteState, RepoState, Result, RevertOutcome, Side, Snapshot, UpstreamGap, Vanished, Worktree,
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

/// The message of a [`Repo::snapshot_files`] snapshot.
const SNAPSHOT: &str = "Brigadier snapshot of the checkout's files";

/// A lock per repository (common git dir), each made on first use and dropped when unused.
type RepoLocks = OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>>;

/// The commits `git log -z --format=%H%x1f%ct%x1f%s` printed.
pub(crate) fn commit_infos(out: &[u8]) -> Result<Vec<CommitInfo>> {
    String::from_utf8_lossy(out)
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

    /// Records `base` as the branch `branch` merges into, in git config
    /// (`branch.<branch>.gh-merge-base`, the key `gh pr create` reads for its base).
    pub fn set_merge_base(&self, branch: &str, base: &str) -> Result<()> {
        self.validate_branch(branch)?;
        self.validate_branch(base)?;
        let key = format!("branch.{branch}.gh-merge-base");
        self.cmd(&["config", key.as_str(), base], false)?;
        Ok(())
    }

    /// The branch `branch` merges into, as [`Self::set_merge_base`] recorded it.
    pub fn merge_base_of(&self, branch: &str) -> Result<Option<String>> {
        self.validate_branch(branch)?;
        let key = format!("branch.{branch}.gh-merge-base");
        let out = self.run(&["config", "--get", key.as_str()], true)?;
        Ok(match out.status.code() {
            Some(0) => Some(parse::line(&out.stdout)?),
            _ => None,
        })
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

    /// Moves this checkout's branch to `to` while its HEAD is still `expected` (`git reset
    /// --keep`): a squash or rebase merge's new base, after its pull request merged. Local
    /// changes it would touch make it fail rather than be lost. Run it under
    /// [`Self::with_ref_lock`].
    pub fn reset_keep(&self, to: &Oid, expected: &Oid) -> Result<()> {
        valid_oid(to)?;
        valid_oid(expected)?;
        if self.resolve("HEAD")? != *expected {
            return Err(Error::Invalid(format!("HEAD is no longer {}", expected.0)));
        }
        self.cmd(&["reset", "--keep", &to.0], false)?;
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
    /// worktree holds; its `branch.<name>.*` config goes with it.
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
        // `git branch -d` drops the branch's config section; `update-ref` doesn't.
        self.remove_branch_config(name)
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
    /// Whether every commit of `tip` is in local branch `into` or in its upstream (`origin/<into>`
    /// when none is set): reachable from it, or there as the same change (a cherry-pick), as
    /// `git cherry` tells. A stale local branch doesn't hide work that landed upstream. A merge
    /// commit of its own has no patch to match (its resolution may be new work): never landed.
    pub fn landed_in(&self, tip: &Oid, into: &str) -> Result<bool> {
        valid_oid(tip)?;
        let mut bases = vec![self.branch_commit(into)?];
        bases.extend(self.upstream_tip(into)?);
        for base in bases {
            let range = format!("{}..{}", base.0, tip.0);
            let merges = self.cmd(
                &["rev-list", "--merges", "--max-count=1", &range, "--"],
                true,
            )?;
            if !merges.trim_ascii().is_empty() {
                continue;
            }
            let out = self.cmd(&["cherry", &base.0, &tip.0], true)?;
            if !out
                .split(|byte| *byte == b'\n')
                .any(|line| line.starts_with(b"+"))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The commit local branch `branch`'s upstream is at, else `origin/<branch>`'s; none when
    /// neither exists.
    pub fn upstream_tip(&self, branch: &str) -> Result<Option<Oid>> {
        self.validate_branch(branch)?;
        for spec in [
            format!("{branch}@{{upstream}}"),
            format!("refs/remotes/origin/{branch}"),
        ] {
            let spec = format!("{spec}^{{commit}}");
            let out = self.run(
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--end-of-options",
                    &spec,
                ],
                true,
            )?;
            if out.status.success() {
                return Ok(Some(parse::oid(&out.stdout)?));
            }
        }
        Ok(None)
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
    /// Build output and caches stay out (#492): an untracked folder a tool tagged with a
    /// `CACHEDIR.TAG`, as cargo tags every target folder, is no file of the project's.
    pub fn snapshot_files(&self) -> Result<Oid> {
        let head = self.resolve("HEAD")?;
        let tree = match self.capture_files(true) {
            Ok((_, tree)) => tree,
            // Unresolved entries or a dirty submodule: the files as they stand all the same.
            Err(_) => {
                let caches = self
                    .status(false)
                    .and_then(|status| self.cache_dirs(&status))
                    .unwrap_or_default();
                self.files_tree_without(&caches)?
            }
        };
        if tree == self.tree_of(&head.0)? {
            return Ok(head);
        }
        self.commit_tree_with(&tree, &[&head], SNAPSHOT, &fixed_identity())
    }

    /// The HEAD a [`Repo::snapshot_files`] snapshot was taken on: its parent, or the snapshot
    /// itself when the files were HEAD's.
    pub fn snapshot_head(&self, snapshot: &Oid) -> Result<Oid> {
        valid_oid(snapshot)?;
        let subject = self.read_only(&["log", "-1", "--format=%s", &snapshot.0])?;
        if subject.trim_end() == SNAPSHOT {
            self.resolve(&format!("{}^", snapshot.0))
        } else {
            Ok(snapshot.clone())
        }
    }

    /// The untracked folders tagged as caches by the Cache Directory Tagging Specification
    /// (a `CACHEDIR.TAG` starting with its signature), outermost only, without tracked files.
    fn cache_dirs(&self, status: &parse::Status) -> Result<Vec<String>> {
        const SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
        let mut dirs: Vec<String> = status
            .untracked()
            .into_iter()
            .filter_map(|path| Some(path.strip_suffix("/CACHEDIR.TAG")?.to_owned()))
            .filter(|dir| {
                let mut head = [0; SIGNATURE.len()];
                fs::File::open(self.root.join(dir).join("CACHEDIR.TAG"))
                    .and_then(|mut tag| std::io::Read::read_exact(&mut tag, &mut head))
                    .is_ok_and(|()| head == SIGNATURE)
            })
            .collect();
        let outer = dirs.clone();
        dirs.retain(|dir| {
            !outer
                .iter()
                .any(|other| dir.starts_with(&format!("{other}/")))
        });
        if dirs.is_empty() {
            return Ok(dirs);
        }
        let mut args = vec!["ls-files", "-z", "--"];
        args.extend(dirs.iter().map(String::as_str));
        let tracked = self.cmd(&args, true)?;
        let tracked: Vec<&[u8]> = tracked
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .collect();
        dirs.retain(|dir| {
            let inside = format!("{dir}/");
            !tracked
                .iter()
                .any(|path| path.starts_with(inside.as_bytes()))
        });
        Ok(dirs)
    }

    /// `add -A` of the whole checkout into a private index, leaving out `skip` (folders).
    fn add_all_but(&self, temp: &TempIndex, skip: &[String]) -> Result<()> {
        let mut args: Vec<OsString> = ["-c", "core.splitIndex=false", "add", "-A", "--", "."]
            .map(OsString::from)
            .to_vec();
        args.extend(
            skip.iter()
                .map(|dir| OsString::from(format!(":(exclude,literal){dir}"))),
        );
        // Exclude magic needs pathspecs read as pathspecs; each path in it stays literal.
        let mut env = temp.env();
        env.push(("GIT_LITERAL_PATHSPECS".into(), "0".into()));
        self.git
            .checked(Some(&self.root), &args, false, &env, None)?;
        Ok(())
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
        self.capture_files(false)
    }

    /// [`Repo::capture`], leaving out untracked cache folders with `skip_caches`.
    fn capture_files(&self, skip_caches: bool) -> Result<(TempIndex, Oid)> {
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
        let caches = if skip_caches {
            self.cache_dirs(&status)?
        } else {
            Vec::new()
        };
        self.add_all_but(&temp, &caches)?;
        let tree = parse::oid(&self.index_cmd(&temp, &["write-tree"])?)?;
        Ok((temp, tree))
    }

    /// The tree of the checkout's files as they stand (HEAD plus every non-ignored change),
    /// read past an index with unresolved entries, which `capture` refuses.
    pub fn files_tree(&self) -> Result<Oid> {
        self.files_tree_without(&[])
    }

    fn files_tree_without(&self, skip: &[String]) -> Result<Oid> {
        let temp = TempIndex::new()?;
        self.index_cmd(&temp, &["read-tree", "HEAD"])?;
        self.add_all_but(&temp, skip)?;
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
    /// Adds, removals, prunes and listings in one repository run one at a time: git reads the
    /// other worktrees' entries while it writes its own, and fails on a half-written one (#207).
    /// Another program's add can't take that lock, so an add that met its half-written entry is
    /// tried again a few times (#268).
    pub fn add_worktree(&self, path: &Path, spec: WorktreeSpec) -> Result<Worktree> {
        let path = absolute(path)?;
        let mut args: Vec<OsString> = vec!["worktree".into(), "add".into()];
        // A failed `add -b` leaves its new branch behind: a retry checks that one out.
        let mut retry_args = None;
        match spec {
            WorktreeSpec::NewBranch { name, start } => {
                self.validate_branch(&name)?;
                valid_oid(&start)?;
                let mut existing = args.clone();
                existing.extend(["--".into(), path.as_os_str().to_owned(), (&name).into()]);
                retry_args = Some((name.clone(), start.clone(), existing));
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
        let lock = self.worktree_lock();
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut attempt = 0;
        loop {
            match self.cmd(&args, false) {
                Err(error) if attempt < 8 && half_written_worktree(&error) => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_millis(25 * attempt));
                    if let Some((name, start, existing)) = &retry_args
                        && self.branch_tip(name)?.as_ref() == Some(start)
                    {
                        args = existing.clone();
                    }
                }
                result => {
                    result?;
                    break;
                }
            }
        }
        drop(guard);
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
        let lock = self.worktree_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if path.try_exists()? {
            let registered = self.list_worktrees()?.iter().any(|w| {
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
        let lock = self.worktree_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.cmd(&["worktree", "prune", "--expire=now"], false)?;
        Ok(())
    }

    /// List registered worktrees using NUL-delimited porcelain metadata.
    pub fn worktrees(&self) -> Result<Vec<WorktreeInfo>> {
        let lock = self.worktree_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.list_worktrees()
    }

    /// [`Self::worktrees`], for a caller already holding the worktree lock.
    fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>> {
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

    /// The ignored files a new worktree of this checkout gets copies of, relative to its root
    /// and sorted: every `.env*` file git ignores outside ignored folders, and what the
    /// checkout's `.worktreeinclude` names (gitignore syntax, as Claude Code reads it) among
    /// the files git ignores. Tracked files never: a worktree has them already. At most `limit`.
    pub fn worktree_copies(&self, limit: usize) -> Result<Vec<String>> {
        let (ignored, _) = self.ignored(limit)?;
        let mut copies: Vec<String> = ignored
            .into_iter()
            .filter(|path| {
                !path.ends_with('/')
                    && Path::new(path)
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(".env"))
            })
            .collect();
        let include = self.root.join(".worktreeinclude");
        if fs::symlink_metadata(&include).is_ok_and(|meta| meta.is_file()) {
            // Read the rules without following a link, then give git a private regular file.
            // Git's exclude-file reader itself follows links.
            let mut options = fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt as _;
                options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
            }
            let mut rules = options.open(&include)?;
            if !rules.metadata()?.is_file() {
                return Err(Error::Invalid(
                    ".worktreeinclude must be a regular file".into(),
                ));
            }
            let patterns = TempIndex::new()?;
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&patterns.path)?;
            std::io::copy(&mut rules, &mut output)?;
            drop(output);
            let from = format!("--exclude-from={}", patterns.path.display());
            let out = self.cmd(&["ls-files", "-z", "--others", "--ignored", &from], true)?;
            for path in out
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty())
                .map(|path| String::from_utf8_lossy(path).into_owned())
                .take(limit)
            {
                if !copies.contains(&path) && self.is_ignored(&path)? {
                    copies.push(path);
                }
            }
        }
        copies.sort();
        copies.truncate(limit);
        Ok(copies)
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

    /// The merges made on the way from `from` to `to`, each as its first parent and a commit it
    /// brought in from elsewhere: every parent but the first of each merge commit on `to`'s
    /// first-parent line after `from` (merges inside what came in are not counted), and the
    /// commit a merge under way brings in (MERGE_HEAD) onto HEAD.
    pub fn merged_in(&self, from: &Oid, to: &Oid) -> Result<Vec<(Oid, Oid)>> {
        valid_oid(from)?;
        valid_oid(to)?;
        let range = format!("{}..{}", from.0, to.0);
        let out = self.cmd(
            &[
                "rev-list",
                "--merges",
                "--first-parent",
                "--parents",
                &range,
                "--",
            ],
            true,
        )?;
        let mut incoming = Vec::new();
        for line in parse::text(&out)?.lines() {
            let mut commits = line.split_whitespace().skip(1);
            let Some(first) = commits.next() else {
                continue;
            };
            let first = parse::oid(first.as_bytes())?;
            for parent in commits {
                incoming.push((first.clone(), parse::oid(parent.as_bytes())?));
            }
        }
        if let (Ok(head), Ok(merging)) = (self.resolve("HEAD"), self.resolve("MERGE_HEAD")) {
            incoming.push((head, merging));
        }
        Ok(incoming)
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

    /// Count commits reachable from `to` but from none of `from`.
    pub fn count_commits_beyond(&self, from: &[&Oid], to: &Oid) -> Result<u32> {
        valid_oid(to)?;
        let mut args = vec!["rev-list".to_owned(), "--count".to_owned(), to.0.clone()];
        for oid in from {
            valid_oid(oid)?;
            args.push(format!("^{}", oid.0));
        }
        args.push("--".into());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        parse::line(&self.cmd(&args, true)?)?
            .parse()
            .map_err(|_| Error::Parse("invalid commit count".into()))
    }

    /// How many commits a merge that made `merge_commit` put on the base, for a branch that
    /// went from `from` to `head`: a merge commit or a fast-forward brings the branch's own; a
    /// rebase merge puts a copy of each on the base (`git cherry` finds them), a squash merge
    /// one for them all.
    pub fn merged_commits(&self, merge_commit: &Oid, from: &Oid, head: &Oid) -> Result<u32> {
        valid_oid(merge_commit)?;
        let commits = self.count_commits(from, head)?;
        let merge_parent = format!("{}^2", merge_commit.0);
        // A merge commit, or a fast-forward: the branch's own commits are on the base.
        if commits <= 1
            || self.resolve(&merge_parent).is_ok()
            || self.merge_base(head, merge_commit)? == *head
        {
            return Ok(commits);
        }
        let cherry = self.cmd(&["cherry", &merge_commit.0, &head.0, &from.0], true)?;
        let copies = String::from_utf8_lossy(&cherry)
            .lines()
            .filter(|line| line.starts_with("- "))
            .count();
        Ok(u32::try_from(copies).unwrap_or(u32::MAX).max(1))
    }

    /// Whether a merge's work is reverted on local branch `base` (or its upstream) now: the
    /// newest commit there since `merge_commit` that reverts one of the merge's commits, or
    /// reverts such a revert, and so on ("This reverts commit <sha>"), is a revert of the work
    /// rather than of a revert. A merge reverted and then restored reads not reverted.
    /// `commits`: how many commits a squash or rebase merge put on the base (a merge commit
    /// brings its second parent's).
    pub fn merge_reverted(&self, merge_commit: &Oid, commits: u32, base: &str) -> Result<bool> {
        Ok(self
            .merge_undo(merge_commit, commits, base)?
            .is_some_and(|undo| undo.redone.is_none()))
    }

    /// What became of a merge's work on local branch `base` (or its upstream) since it merged,
    /// read as [`Self::merge_reverted`] reads it: the newest revert of the work, with the
    /// commit that put it back after that (a revert of the revert), if one did. None: never
    /// reverted. Times are the commits' own commit times.
    pub fn merge_undo(
        &self,
        merge_commit: &Oid,
        commits: u32,
        base: &str,
    ) -> Result<Option<crate::MergeUndo>> {
        valid_oid(merge_commit)?;
        let mut tips = vec![self.branch_commit(base)?];
        tips.extend(self.upstream_tip(base)?);
        let parents = parse::line(&self.cmd(
            &[
                "rev-list",
                "--parents",
                "--max-count=1",
                &merge_commit.0,
                "--",
            ],
            true,
        )?)?
        .split_whitespace()
        .count()
        .saturating_sub(1);
        let brought = if parents > 1 {
            self.cmd(
                &["rev-list", &format!("{0}^1..{0}", merge_commit.0), "--"],
                true,
            )?
        } else {
            self.cmd(
                &[
                    "rev-list",
                    "--first-parent",
                    &format!("--max-count={}", commits.max(1)),
                    &merge_commit.0,
                    "--",
                ],
                true,
            )?
        };
        // Whether each known commit puts the work in (true) or takes it out (false).
        let mut puts_in: HashMap<String, bool> = String::from_utf8_lossy(&brought)
            .lines()
            .map(|sha| (sha.trim().to_owned(), true))
            .collect();
        let mut args = vec![
            "log".to_owned(),
            "--reverse".to_owned(),
            "--topo-order".to_owned(),
            "--format=%H%x00%ct%x00%B%x1e".to_owned(),
            format!("^{}", merge_commit.0),
        ];
        args.extend(tips.iter().map(|tip| tip.0.clone()));
        args.push("--".to_owned());
        let log = self.cmd(&args, true)?;
        let mut undo: Option<crate::MergeUndo> = None;
        for entry in String::from_utf8_lossy(&log).split('\x1e') {
            let mut fields = entry.trim_start_matches('\n').splitn(3, '\0');
            let (Some(sha), Some(time), Some(body)) = (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let undone = body.lines().find_map(|line| {
                let named = line.trim().strip_prefix("This reverts commit ")?;
                let named = named.split(|c: char| !c.is_ascii_hexdigit()).next()?;
                puts_in.get(named).copied()
            });
            if let Some(undone) = undone {
                puts_in.insert(sha.to_owned(), !undone);
                let at = (
                    Oid(sha.to_owned()),
                    time.trim().parse::<i64>().unwrap_or_default() * 1000,
                );
                if undone {
                    undo = Some(crate::MergeUndo {
                        reverted: at,
                        redone: None,
                    });
                } else if let Some(undo) = &mut undo {
                    undo.redone = Some(at);
                }
            }
        }
        Ok(undo)
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

    /// The branch checked out here; `None` on a detached `HEAD`.
    pub fn current_branch(&self) -> Result<Option<String>> {
        self.symbolic_head()
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
        if let Some(what) = self.busy_besides_merge()? {
            return Ok(Some(what));
        }
        let dir = parse::path_line(&self.cmd(&["rev-parse", "--absolute-git-dir"], true)?)?;
        Ok(dir
            .join("MERGE_HEAD")
            .try_exists()?
            .then(|| "MERGE_HEAD".to_owned()))
    }

    /// What keeps the checkout busy other than a merge in progress, which the user's own
    /// staging and commit go on through.
    pub(crate) fn busy_besides_merge(&self) -> Result<Option<String>> {
        self.busy_except(&[])
    }

    /// What keeps the checkout busy other than a merge or rebase in progress, whose conflicts
    /// the user stages, unstages and discards.
    pub(crate) fn busy_besides_conflicts(&self) -> Result<Option<String>> {
        self.busy_except(&["rebase-merge", "rebase-apply"])
    }

    fn busy_except(&self, allowed: &[&str]) -> Result<Option<String>> {
        let dir = parse::path_line(&self.cmd(&["rev-parse", "--absolute-git-dir"], true)?)?;
        for name in [
            "index.lock",
            "HEAD.lock",
            "rebase-merge",
            "rebase-apply",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "BISECT_LOG",
            "BISECT_START",
            "sequencer",
        ] {
            if !allowed.contains(&name) && dir.join(name).try_exists()? {
                return Ok(Some(match name {
                    "rebase-merge" | "rebase-apply" => "a rebase in progress".to_owned(),
                    _ => name.to_owned(),
                }));
            }
        }
        Ok(None)
    }

    pub(crate) fn landing_lock(&self) -> Arc<Mutex<()>> {
        static LOCKS: RepoLocks = OnceLock::new();
        self.lock_in(&LOCKS)
    }

    /// The lock worktree adds, removals, prunes and listings of this repository take.
    fn worktree_lock(&self) -> Arc<Mutex<()>> {
        static LOCKS: RepoLocks = OnceLock::new();
        self.lock_in(&LOCKS)
    }

    /// This repository's lock (per common git dir) in `locks`.
    fn lock_in(&self, locks: &RepoLocks) -> Arc<Mutex<()>> {
        let mut locks = locks
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

    /// The checkout rebasing local branch `branch`, if any.
    pub fn rebasing_checkout(&self, branch: &str) -> Result<Option<PathBuf>> {
        self.rebasing_in(&self.worktrees()?, branch)
    }

    /// [`Self::rebasing_checkout`] among `worktrees`: only a detached one can be rebasing.
    fn rebasing_in(&self, worktrees: &[WorktreeInfo], branch: &str) -> Result<Option<PathBuf>> {
        for worktree in worktrees {
            if worktree.branch.is_some() || worktree.prunable || !worktree.path.is_dir() {
                continue;
            }
            let checkout = Repo {
                root: worktree.path.clone(),
                ..self.clone()
            };
            if checkout.rebased_branch().ok().flatten().as_deref() == Some(branch) {
                return Ok(Some(worktree.path.clone()));
            }
        }
        Ok(None)
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
        let worktrees = self.worktrees()?;
        // A checkout rebasing the branch has HEAD detached until the rebase finishes, which
        // would then overwrite whatever landed meanwhile.
        if let Some(worktree) = self.rebasing_in(&worktrees, &request.branch)? {
            return Ok(LandOutcome::Blocked(LandBlock::CheckoutBusy {
                worktree,
                what: format!("a rebase of {} in progress", request.branch),
            }));
        }
        let holding = worktrees
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
        self.review_patch_of(from, to, context, ignore_whitespace, &[])
    }

    /// [`Self::review_patch`] of `paths` only; every path when none are given.
    pub fn review_patch_of(
        &self,
        from: &Oid,
        to: &Oid,
        context: u32,
        ignore_whitespace: bool,
        paths: &[String],
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
        if !paths.is_empty() {
            args.insert(0, "--literal-pathspecs");
            args.extend(paths.iter().map(String::as_str));
        }
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
        commit_infos(&out)
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
    ///
    /// During a merge it commits the merge, once no file is left conflicted; `amend` instead
    /// rewrites the last commit with what is staged, keeping its message when `message` is
    /// blank.
    pub fn commit_changes(&self, message: &str, include_unstaged: bool) -> Result<Oid> {
        self.commit_with(message, include_unstaged, false)
    }

    /// [`Self::commit_changes`], or with `amend` the last commit rewritten.
    pub fn commit_with(&self, message: &str, include_unstaged: bool, amend: bool) -> Result<Oid> {
        let lock = self.landing_lock();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.rebased_branch()?.is_some() {
            return Err(Error::Invalid(
                "continue or abort the rebase in progress first".into(),
            ));
        }
        if let Some(what) = self.busy_besides_merge()? {
            return Err(Error::Invalid(format!("the checkout is busy ({what})")));
        }
        if self.merging()?.is_some() {
            if include_unstaged || amend {
                return Err(Error::Invalid(
                    "finish or abort the merge in progress first".into(),
                ));
            }
            if !self.cmd(&["ls-files", "--unmerged"], true)?.is_empty() {
                return Err(Error::Invalid(
                    "resolve the merge conflicts and stage those files first".into(),
                ));
            }
        }
        if include_unstaged {
            self.cmd(&["add", "-A", "--", "."], false)?;
        }
        let mut args = vec!["commit", "--quiet"];
        if amend {
            args.push("--amend");
        }
        if amend && message.trim().is_empty() {
            args.push("--no-edit");
            self.git
                .checked(Some(&self.root), &args, false, &[], None)?;
        } else {
            args.push("--file=-");
            self.git.checked(
                Some(&self.root),
                &args,
                false,
                &[],
                Some(message.as_bytes()),
            )?;
        }
        self.resolve("HEAD")
    }

    /// Whether `branch` has an upstream set (`push -u`, `branch --set-upstream-to`), whether
    /// or not its remote-tracking ref is here: a clone that fetches only main has none.
    pub fn upstream_configured(&self, branch: &str) -> Result<bool> {
        self.validate_branch(branch)?;
        let key = format!("branch.{branch}.merge");
        let out = self.run(&["config", "--get", key.as_str()], true)?;
        Ok(out.status.success())
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

    /// Fetches the remote `branch` pushes to (its upstream's, else `origin`), asking nothing
    /// on a terminal; git is stopped after `limit`. Touches no checkout or local branch. No
    /// remote: nothing to fetch.
    pub fn fetch_remote_of(&self, branch: &str, limit: std::time::Duration) -> Result<()> {
        let Some(remote) = self.remote_state(branch)?.remote else {
            return Ok(());
        };
        let args = ["fetch", "--quiet", "--no-tags", "--end-of-options", &remote];
        let env: Environment = vec![("GIT_TERMINAL_PROMPT".into(), "0".into())];
        self.git
            .checked_within(Some(&self.root), &args, false, &env, limit)?;
        Ok(())
    }

    /// Local `branch` against its upstream as last fetched: the one it tracks, else
    /// `origin/<branch>` when it tracks none. None without one, or when the one it tracks
    /// isn't fetched (`origin`'s isn't taken for it then).
    pub fn upstream_gap(&self, branch: &str) -> Result<Option<UpstreamGap>> {
        self.validate_branch(branch)?;
        let tracks = self
            .run(
                &["config", "--get", &format!("branch.{branch}.merge")],
                true,
            )?
            .status
            .success();
        let (name, spec) = match self.remote_state(branch)?.upstream {
            Some(name) => (name, format!("{branch}@{{upstream}}")),
            None if tracks => return Ok(None),
            None => (
                format!("origin/{branch}"),
                format!("refs/remotes/origin/{branch}"),
            ),
        };
        let out = self.run(
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "--end-of-options",
                &format!("{spec}^{{commit}}"),
            ],
            true,
        )?;
        if !out.status.success() {
            return Ok(None);
        }
        let upstream = parse::oid(&out.stdout)?;
        let tip = self
            .branch_tip(branch)?
            .ok_or_else(|| Error::Invalid(format!("no branch `{branch}`")))?;
        let range = format!("{}...{}", tip.0, upstream.0);
        let counts =
            parse::line(&self.cmd(&["rev-list", "--left-right", "--count", &range, "--"], true)?)?;
        let mut counts = counts.split_whitespace().map(str::parse::<u32>);
        let (Some(Ok(ahead)), Some(Ok(behind))) = (counts.next(), counts.next()) else {
            return Err(Error::Parse("invalid commit counts".into()));
        };
        Ok(Some(UpstreamGap {
            name,
            tip: upstream,
            ahead,
            behind,
        }))
    }

    /// Where new work from local `branch` starts: its upstream's tip as last fetched when
    /// local `branch` is only behind it (someone else pushed), with that gap; else local
    /// `branch`'s own tip (ahead, diverged, level, or no upstream: the user's commits win).
    pub fn newer_tip(&self, branch: &str) -> Result<(Oid, Option<UpstreamGap>)> {
        let local = self.branch_commit(branch)?;
        Ok(match self.upstream_gap(branch)? {
            Some(gap) if gap.ahead == 0 && gap.behind > 0 => (gap.tip.clone(), Some(gap)),
            _ => (local, None),
        })
    }

    /// Pushes `branch` to its upstream, or to `origin` as its new upstream. Never forces, and
    /// git asks nothing on a terminal.
    pub fn push(&self, branch: &str) -> Result<()> {
        self.push_leased(branch, None)
    }

    /// [`Self::push`]; when that is refused and local `branch` starts from `merge_commit`, the
    /// commit a pull request's squash or rebase merge made from its head `merged`, a push
    /// that replaces the remote branch only while it is still exactly `merged` (#294): the
    /// commits GitHub kept on a merged pull request's branch, nothing anyone pushed since.
    /// The plain push's refusal stands otherwise.
    pub fn push_after_merge(&self, branch: &str, merged: &Oid, merge_commit: &Oid) -> Result<()> {
        let Err(refused) = self.push(branch) else {
            return Ok(());
        };
        let from_merge = self
            .branch_commit(branch)
            .and_then(|tip| self.ancestor(merge_commit, &tip));
        if !matches!(from_merge, Ok(true)) {
            return Err(refused);
        }
        self.push_leased(branch, Some(merged)).map_err(|_| refused)
    }

    /// Whether local `branch` starts from `merge_commit`, the commit a pull request's squash
    /// or rebase merge made from its head `merged`, and its upstream as last fetched is still
    /// exactly `merged`: the commits GitHub kept on the merged pull request's branch, which a
    /// pull would merge back (#331). Not when either can't be read.
    pub fn kept_after_merge(&self, branch: &str, merged: &Oid, merge_commit: &Oid) -> bool {
        let from_merge = self
            .branch_commit(branch)
            .and_then(|tip| self.ancestor(merge_commit, &tip));
        matches!(from_merge, Ok(true))
            && matches!(self.upstream_gap(branch), Ok(Some(gap)) if gap.tip == *merged)
    }

    fn push_leased(&self, branch: &str, lease: Option<&Oid>) -> Result<()> {
        let state = self.remote_state(branch)?;
        let remote = state
            .remote
            .ok_or_else(|| Error::Invalid("the repository has no remote to push to".into()))?;
        let refspec = format!("refs/heads/{branch}");
        let lease = match lease {
            Some(oid) => {
                valid_oid(oid)?;
                Some(format!("--force-with-lease={refspec}:{}", oid.0))
            }
            None => None,
        };
        let mut args = vec!["push", "--porcelain"];
        if state.upstream.is_none() {
            args.push("--set-upstream");
        }
        args.extend(lease.as_deref());
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
pub(crate) fn copy_index(from: &Path, to: &Path) -> Result<()> {
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

/// Whether git failed on another worktree's entry that was still being written.
fn half_written_worktree(error: &Error) -> bool {
    matches!(error, Error::Command { stderr, .. }
        if stderr.contains("failed to read") && stderr.contains("worktrees/"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::vanished;
    use crate::{Git, Oid};
    use std::{ffi::OsString, fs, path::PathBuf};

    /// A folder in the temp directory, removed when dropped however the test ends.
    pub(crate) struct Temp(PathBuf);

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
    pub(crate) fn test_git(name: &str) -> (Temp, Git) {
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

    /// "Last turn" over several landings shows each of them and nothing that landed between
    /// them; "uncommitted" shows staged and unstaged edits together (Q90's `open_in_app`).
    #[test]
    fn a_requests_landings_and_a_checkouts_uncommitted_edits_read_whole() {
        let (dir, git) = test_git("review-scopes");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        repo.cmd(&["checkout", "-q", "-b", "main"], false).unwrap();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        repo.commit_changes("Start", true).unwrap();
        fs::write(root.join("b.txt"), "first landing\n").unwrap();
        repo.commit_changes("First landing", true).unwrap();
        let first = repo.resolve("HEAD").unwrap();
        fs::write(root.join("other.txt"), "someone else\n").unwrap();
        repo.commit_changes("Another request", true).unwrap();
        fs::write(root.join("c.txt"), "second landing\n").unwrap();
        repo.commit_changes("Second landing", true).unwrap();
        let second = repo.resolve("HEAD").unwrap();
        let (from, to) = repo.changes_of(&[second, first]).unwrap();
        let patch = repo.review_patch(&from, &to, 3, false).unwrap();
        assert!(patch.contains("+first landing"), "{patch}");
        assert!(patch.contains("+second landing"), "{patch}");
        assert!(!patch.contains("someone else"), "{patch}");

        fs::write(root.join("a.txt"), "a\nstaged\n").unwrap();
        repo.cmd(&["add", "a.txt"], false).unwrap();
        fs::write(root.join("b.txt"), "first landing\nunstaged\n").unwrap();
        let trees = repo.checkout_trees().unwrap();
        let patch = repo
            .review_patch(&trees.head_tree, &trees.files, 3, false)
            .unwrap();
        assert!(patch.contains("+staged"), "{patch}");
        assert!(patch.contains("+unstaged"), "{patch}");
    }

    /// A merge reads reverted after a revert of it, merged again after the revert is
    /// reverted, and reverted once more after that; a squash merge's revert counts as one of a
    /// merge commit does, and unrelated commits since change nothing.
    #[test]
    fn a_merge_reverted_then_restored_reads_as_the_latest_revert_leaves_it() {
        let (dir, git) = test_git("merge-reverted");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let run = |args: &[&str]| repo.cmd(args, false).expect("git");
        run(&["checkout", "-q", "-b", "main"]);
        fs::write(root.join("a.txt"), "a\n").unwrap();
        repo.commit_changes("Start", true).unwrap();
        // A merge commit of a branch with two commits.
        run(&["checkout", "-q", "-b", "work"]);
        fs::write(root.join("b.txt"), "b\n").unwrap();
        repo.commit_changes("Add b", true).unwrap();
        fs::write(root.join("c.txt"), "c\n").unwrap();
        repo.commit_changes("Add c", true).unwrap();
        run(&["checkout", "-q", "main"]);
        run(&["merge", "-q", "--no-ff", "-m", "Merge work", "work"]);
        let merge = repo.resolve("HEAD").unwrap();
        let reverted = || repo.merge_reverted(&merge, 2, "main").unwrap();
        assert!(!reverted(), "just merged");
        assert_eq!(repo.merge_undo(&merge, 2, "main").unwrap(), None);
        fs::write(root.join("d.txt"), "d\n").unwrap();
        repo.commit_changes("Unrelated", true).unwrap();
        assert!(!reverted());
        run(&["revert", "--no-edit", "-m", "1", &merge.0]);
        assert!(reverted(), "reverted");
        let revert = repo.resolve("HEAD").unwrap();
        run(&["revert", "--no-edit", &revert.0]);
        assert!(!reverted(), "the revert reverted: merged again");
        let restore = repo.resolve("HEAD").unwrap();
        let undo = repo.merge_undo(&merge, 2, "main").unwrap().unwrap();
        assert_eq!(undo.reverted.0, revert, "the revert");
        assert_eq!(undo.redone.as_ref().map(|redone| &redone.0), Some(&restore));
        assert!(undo.reverted.1 > 0, "its time");
        run(&["revert", "--no-edit", &restore.0]);
        assert!(reverted(), "reverted once more");
        let again = repo.resolve("HEAD").unwrap();
        let undo = repo.merge_undo(&merge, 2, "main").unwrap().unwrap();
        assert_eq!(
            (undo.reverted.0, undo.redone),
            (again, None),
            "the newest revert"
        );

        // A squash merge: one commit on the base.
        run(&["checkout", "-q", "-b", "more"]);
        fs::write(root.join("e.txt"), "e\n").unwrap();
        repo.commit_changes("Add e", true).unwrap();
        run(&["checkout", "-q", "main"]);
        run(&["merge", "-q", "--squash", "more"]);
        let squash = repo.commit_changes("Squash more", true).unwrap();
        assert!(!repo.merge_reverted(&squash, 1, "main").unwrap());
        run(&["revert", "--no-edit", &squash.0]);
        assert!(repo.merge_reverted(&squash, 1, "main").unwrap());
    }

    /// A squash merge of a branch with two commits puts one on the base, a rebase merge a
    /// copy of each and a merge commit brings both; so a revert of the base's commit before a
    /// squash doesn't read as a revert of the merge.
    #[test]
    fn a_merge_counts_the_commits_it_put_on_the_base() {
        let (dir, git) = test_git("merged-commits");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let run = |args: &[&str]| repo.cmd(args, false).expect("git");
        run(&["checkout", "-q", "-b", "main"]);
        fs::write(root.join("a.txt"), "a\n").unwrap();
        let start = repo.commit_changes("Start", true).unwrap();
        run(&["checkout", "-q", "-b", "work"]);
        fs::write(root.join("b.txt"), "b\n").unwrap();
        repo.commit_changes("Add b", true).unwrap();
        fs::write(root.join("c.txt"), "c\n").unwrap();
        let head = repo.commit_changes("Add c", true).unwrap();
        run(&["checkout", "-q", "main"]);
        fs::write(root.join("d.txt"), "d\n").unwrap();
        let before = repo.commit_changes("Unrelated", true).unwrap();

        run(&["merge", "-q", "--squash", "work"]);
        let squash = repo.commit_changes("Work (#7)", true).unwrap();
        assert_eq!(
            repo.merged_commits(&squash, &start, &head).unwrap(),
            1,
            "a squash"
        );
        run(&["revert", "--no-edit", &before.0]);
        assert!(
            !repo.merge_reverted(&squash, 1, "main").unwrap(),
            "the commit before the squash is no part of it"
        );

        run(&["reset", "-q", "--hard", &before.0]);
        run(&["cherry-pick", &format!("{}..{}", start.0, head.0)]);
        let rebased = repo.resolve("HEAD").unwrap();
        assert_eq!(
            repo.merged_commits(&rebased, &start, &head).unwrap(),
            2,
            "a rebase"
        );

        run(&["reset", "-q", "--hard", &before.0]);
        run(&["merge", "-q", "--no-ff", "-m", "Merge work", "work"]);
        let merge = repo.resolve("HEAD").unwrap();
        assert_eq!(
            repo.merged_commits(&merge, &start, &head).unwrap(),
            2,
            "a merge commit"
        );
        assert_eq!(
            repo.merged_commits(&head, &start, &head).unwrap(),
            2,
            "a fast-forward"
        );
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

    /// Build output tagged as a cache (cargo's target folder, wherever it is) is no file of
    /// the checkout's snapshot; an untagged folder, a tag without the signature and a tagged
    /// folder with tracked files still are (#492).
    #[test]
    fn a_snapshot_leaves_out_untracked_cache_folders() {
        let (dir, git) = test_git("cachedir");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "a\n").unwrap();
        fs::create_dir_all(root.join("kept")).unwrap();
        fs::write(root.join("kept/CACHEDIR.TAG"), CACHE_TAG).unwrap();
        fs::write(root.join("kept/tracked.txt"), "t\n").unwrap();
        let head = repo.commit_changes("Base", true).unwrap();
        let tag = |dir: &str| {
            fs::create_dir_all(root.join(dir).join("debug/deps")).unwrap();
            fs::write(root.join(dir).join("CACHEDIR.TAG"), CACHE_TAG).unwrap();
            fs::write(root.join(dir).join("debug/deps/lib.rlib"), "built").unwrap();
        };
        tag("target-base");
        tag("nested/out");
        tag("target-base/inner");
        fs::create_dir_all(root.join("fake")).unwrap();
        fs::write(root.join("fake/CACHEDIR.TAG"), "not a tag").unwrap();
        fs::create_dir_all(root.join("scratch")).unwrap();
        fs::write(root.join("scratch/x.txt"), "x\n").unwrap();
        fs::write(root.join("kept/new.txt"), "n\n").unwrap();
        fs::write(root.join("a.txt"), "a2\n").unwrap();
        let snapshot = repo.snapshot_files().unwrap();
        let files: Vec<_> = repo.file_blobs(&snapshot).unwrap().into_keys().collect();
        assert_eq!(
            files,
            [
                "a.txt",
                "fake/CACHEDIR.TAG",
                "kept/CACHEDIR.TAG",
                "kept/new.txt",
                "kept/tracked.txt",
                "scratch/x.txt"
            ]
        );
        assert_eq!(repo.snapshot_head(&snapshot).unwrap(), head);
        // Only the tagged folders changed since HEAD: the snapshot is HEAD itself.
        fs::write(root.join("a.txt"), "a\n").unwrap();
        for path in ["fake", "scratch"] {
            fs::remove_dir_all(root.join(path)).unwrap();
        }
        fs::remove_file(root.join("kept/new.txt")).unwrap();
        assert_eq!(repo.snapshot_files().unwrap(), head);
        assert_eq!(repo.snapshot_head(&head).unwrap(), head);
    }

    const CACHE_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# a cache\n";

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

    #[test]
    fn work_on_the_upstream_main_or_cherry_picked_there_has_landed() {
        let (dir, git) = test_git("landed-in");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let run = |args: &[&str]| repo.cmd(args, false).expect("git");
        fs::write(root.join("a.txt"), "a\n").expect("a file");
        repo.commit_changes("Base", true).expect("a commit");
        run(&["branch", "-M", "main"]);
        // Work on a branch, then cherry-picked onto the upstream main: the local main is stale.
        run(&["checkout", "--quiet", "-b", "picked"]);
        fs::write(root.join("b.txt"), "b\n").expect("a file");
        let picked = repo.commit_changes("Add b", true).expect("a commit");
        run(&["checkout", "--quiet", "-b", "upstream", "main"]);
        fs::write(root.join("c.txt"), "c\n").expect("a file");
        repo.commit_changes("Add c upstream", true)
            .expect("a commit");
        run(&["cherry-pick", picked.0.as_str()]);
        let upstream = repo.resolve("HEAD").expect("HEAD");
        run(&["checkout", "--quiet", "main"]);
        let landed = |tip: &Oid| repo.landed_in(tip, "main").expect("landed");
        assert!(!landed(&picked), "no upstream: the stale main lacks it");
        run(&[
            "update-ref",
            "refs/remotes/origin/main",
            upstream.0.as_str(),
        ]);
        assert_eq!(
            repo.upstream_tip("main").expect("upstream"),
            Some(upstream.clone())
        );
        assert!(landed(&picked), "cherry-picked onto origin/main");
        assert!(
            landed(&upstream),
            "equal to origin/main, ahead of the stale main"
        );
        run(&["checkout", "--quiet", "-b", "fresh", "main"]);
        fs::write(root.join("d.txt"), "d\n").expect("a file");
        let fresh = repo.commit_changes("Add d", true).expect("a commit");
        assert!(!landed(&fresh), "new work landed nowhere");
        // A merge of its own, even of commits already upstream, may hold a resolution of its own.
        run(&["checkout", "--quiet", "-b", "merged", "main"]);
        run(&[
            "merge",
            "--quiet",
            "--no-ff",
            "--no-edit",
            picked.0.as_str(),
        ]);
        let merged = repo.resolve("HEAD").expect("HEAD");
        assert!(!landed(&merged), "its merge commit is nowhere upstream");
    }

    #[test]
    fn a_fetch_shows_how_far_main_is_from_its_upstream() {
        let (dir, git) = test_git("upstream-gap");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let run = |args: &[&str]| repo.cmd(args, false).expect("git");
        fs::write(root.join("a.txt"), "a\n").expect("a file");
        repo.commit_changes("Base", true).expect("a commit");
        run(&["branch", "-M", "main"]);
        assert_eq!(repo.upstream_gap("main").expect("gap"), None);
        let limit = std::time::Duration::from_secs(20);
        repo.fetch_remote_of("main", limit)
            .expect("no remote: nothing to fetch");
        let origin = dir.join("origin.git");
        run(&[
            "clone",
            "--quiet",
            "--bare",
            ".",
            origin.to_str().expect("path"),
        ]);
        run(&["remote", "add", "origin", origin.to_str().expect("path")]);
        run(&["fetch", "--quiet", "origin"]);
        run(&[
            "branch",
            "--quiet",
            "--set-upstream-to",
            "origin/main",
            "main",
        ]);
        // Someone else pushes to origin's main; this main commits on its own.
        let other = dir.join("other");
        run(&[
            "clone",
            "--quiet",
            origin.to_str().expect("path"),
            other.to_str().expect("path"),
        ]);
        let other = git.open(&other).expect("the clone");
        fs::write(other.root().join("b.txt"), "b\n").expect("a file");
        other.commit_changes("Add b", true).expect("a commit");
        other
            .cmd(&["push", "--quiet", "origin", "HEAD:main"], false)
            .expect("a push");
        let gap = repo
            .upstream_gap("main")
            .expect("gap")
            .expect("an upstream");
        assert_eq!((gap.ahead, gap.behind), (0, 0), "not fetched yet");
        repo.fetch_remote_of("main", limit).expect("a fetch");
        let gap = repo
            .upstream_gap("main")
            .expect("gap")
            .expect("an upstream");
        assert_eq!(gap.name, "origin/main");
        assert_eq!((gap.ahead, gap.behind), (0, 1));
        fs::write(root.join("c.txt"), "c\n").expect("a file");
        repo.commit_changes("Add c", true).expect("a commit");
        let gap = repo
            .upstream_gap("main")
            .expect("gap")
            .expect("an upstream");
        assert_eq!((gap.ahead, gap.behind), (1, 1), "diverged");
        // Tracking a remote that isn't fetched: no gap, and origin's main isn't taken for it.
        run(&["remote", "add", "fork", origin.to_str().expect("path")]);
        run(&["config", "branch.main.remote", "fork"]);
        assert_eq!(repo.upstream_gap("main").expect("gap"), None);
    }

    #[test]
    fn a_new_worktree_gets_the_ignored_env_files_and_what_worktreeinclude_names() {
        let (dir, git) = test_git("worktree-copies");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
            fs::write(path, text).expect("a file");
        };
        write(
            ".gitignore",
            ".env*\n!.env.example\nnode_modules/\nconfig/local.json\nbuild.log\n",
        );
        write(".env.example", "KEY=\n");
        repo.commit_changes("Base", true).expect("a commit");
        write(".env", "KEY=one\n");
        write(".env.local", "KEY=two\n");
        write("apps/web/.env", "KEY=three\n");
        write("node_modules/pkg/.env", "KEY=four\n");
        write("config/local.json", "{}\n");
        write("build.log", "log\n");
        write("notes.txt", "not ignored\n");
        assert_eq!(
            repo.worktree_copies(100).expect("copies"),
            [".env", ".env.local", "apps/web/.env"],
        );
        // `.worktreeinclude` adds ignored files; one git doesn't ignore is never taken.
        write(".worktreeinclude", "config/local.json\nnotes.txt\n");
        assert_eq!(
            repo.worktree_copies(100).expect("copies"),
            [".env", ".env.local", "apps/web/.env", "config/local.json"],
        );
    }

    #[cfg(unix)]
    #[test]
    fn worktree_copies_do_not_follow_a_linked_include_file_or_directory() {
        use std::os::unix::fs::symlink;
        let (dir, git) = test_git("20261017-2108-workspaces-rework-w54-linked-include");
        let root = dir.join("repo");
        git.init(&root).unwrap();
        let repo = git.open(&root).unwrap();
        fs::write(root.join(".gitignore"), ".env*\nconfig/\n").unwrap();
        fs::write(root.join(".env"), "KEY=local\n").unwrap();
        fs::create_dir(root.join("config")).unwrap();
        fs::write(root.join("config/local"), "local").unwrap();
        let outside = dir.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("rules"), "config/\n").unwrap();
        fs::write(outside.join("token"), "outside").unwrap();
        symlink(outside.join("rules"), root.join(".worktreeinclude")).unwrap();
        assert_eq!(repo.worktree_copies(100).unwrap(), [".env"]);
        fs::remove_file(root.join(".worktreeinclude")).unwrap();
        fs::write(root.join(".worktreeinclude"), "config/\n").unwrap();
        symlink(&outside, root.join("config/linked")).unwrap();
        assert_eq!(
            repo.worktree_copies(100).unwrap(),
            [".env", "config/linked", "config/local"]
        );
    }

    #[test]
    fn new_work_starts_from_the_newer_of_main_and_its_upstream() {
        let (dir, git) = test_git("newer-tip");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let run = |args: &[&str]| repo.cmd(args, false).expect("git");
        fs::write(root.join("a.txt"), "a\n").expect("a file");
        let base = repo.commit_changes("Base", true).expect("a commit");
        run(&["branch", "-M", "main"]);
        // No remote: the local tip.
        assert_eq!(repo.newer_tip("main").expect("tip"), (base.clone(), None));
        let origin = dir.join("origin.git");
        run(&[
            "clone",
            "--quiet",
            "--bare",
            ".",
            origin.to_str().expect("path"),
        ]);
        run(&["remote", "add", "origin", origin.to_str().expect("path")]);
        run(&["fetch", "--quiet", "origin"]);
        // Level with origin, tracking nothing: the local tip.
        assert_eq!(repo.newer_tip("main").expect("tip"), (base.clone(), None));
        let other = dir.join("other");
        run(&[
            "clone",
            "--quiet",
            origin.to_str().expect("path"),
            other.to_str().expect("path"),
        ]);
        let other = git.open(&other).expect("the clone");
        fs::write(other.root().join("b.txt"), "b\n").expect("a file");
        let pushed = other.commit_changes("Add b", true).expect("a commit");
        other
            .cmd(&["push", "--quiet", "origin", "HEAD:main"], false)
            .expect("a push");
        repo.fetch_remote_of("main", std::time::Duration::from_secs(20))
            .expect("a fetch");
        // Only behind: origin's tip, with how far.
        let (tip, gap) = repo.newer_tip("main").expect("tip");
        assert_eq!(tip, pushed);
        let gap = gap.expect("the gap");
        assert_eq!((gap.name.as_str(), gap.behind), ("origin/main", 1));
        // Diverged: the user's own commits win.
        fs::write(root.join("c.txt"), "c\n").expect("a file");
        let own = repo.commit_changes("Add c", true).expect("a commit");
        assert_eq!(repo.newer_tip("main").expect("tip"), (own, None));
    }

    #[test]
    fn a_helper_left_holding_gits_output_doesnt_hold_the_fetch() {
        let (dir, git) = test_git("fetch-helper");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let run = |args: &[&str]| repo.cmd(args, false).expect("git");
        run(&["branch", "-M", "main"]);
        run(&["remote", "add", "origin", "ssh://example.invalid/r.git"]);
        // The ssh stand-in fails at once and leaves a child writing to git's stderr.
        run(&["config", "core.sshCommand", "sleep 8 >&2 & exit 1;"]);
        let started = std::time::Instant::now();
        let err = repo
            .fetch_remote_of("main", std::time::Duration::from_secs(20))
            .expect_err("the fetch fails");
        assert!(!err.to_string().contains("was stopped"), "{err}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_fetch_that_hangs_is_stopped_at_its_limit() {
        let (dir, git) = test_git("fetch-limit");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        let run = |args: &[&str]| repo.cmd(args, false).expect("git");
        run(&["branch", "-M", "main"]);
        run(&["remote", "add", "origin", "ssh://example.invalid/r.git"]);
        // A marker in its command line finds what it started, if anything was left running.
        let marker = format!("sleep 30 # {}", dir.display());
        run(&["config", "core.sshCommand", &format!("{marker}\n")]);
        let started = std::time::Instant::now();
        let err = repo
            .fetch_remote_of("main", std::time::Duration::from_secs(1))
            .expect_err("stopped");
        assert!(err.to_string().contains("was stopped"), "{err}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        #[cfg(unix)]
        {
            let left = std::process::Command::new("pgrep")
                .args(["-f", &marker])
                .output()
                .expect("pgrep");
            assert!(left.stdout.is_empty(), "left running: {left:?}");
        }
    }

    #[test]
    fn eight_worktrees_added_at_once_in_one_repository_all_start_while_others_list_them() {
        let (dir, git) = test_git("concurrent-adds");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "a\n").expect("a file");
        let base = repo.commit_changes("Base", true).expect("a commit");
        for round in 0..6 {
            let start = std::sync::Arc::new(std::sync::Barrier::new(12));
            let lists: Vec<_> = (0..4)
                .map(|_| {
                    let (git, root, start) = (git.clone(), root.clone(), start.clone());
                    std::thread::spawn(move || {
                        let repo = git.open(&root).expect("the repository");
                        start.wait();
                        (0..4).try_for_each(|_| repo.worktrees().map(|_| ()))
                    })
                })
                .collect();
            let adds: Vec<_> = (0..8)
                .map(|index| {
                    let (git, root, start, base) =
                        (git.clone(), root.clone(), start.clone(), base.clone());
                    let path = dir.join(format!("wt-{round}-{index}"));
                    std::thread::spawn(move || {
                        // Each its own handle, as separate sessions open the repository.
                        let repo = git.open(&root).expect("the repository");
                        start.wait();
                        let spec = if index % 2 == 0 {
                            crate::WorktreeSpec::Detached { at: base }
                        } else {
                            crate::WorktreeSpec::NewBranch {
                                name: format!("task-{round}-{index}"),
                                start: base,
                            }
                        };
                        repo.add_worktree(&path, spec).map(|_| ())
                    })
                })
                .collect();
            for add in adds {
                add.join()
                    .expect("no panic")
                    .expect("the worktree is added");
            }
            for list in lists {
                list.join()
                    .expect("no panic")
                    .expect("the worktrees are listed");
            }
        }
        assert_eq!(repo.worktrees().expect("the worktrees").len(), 49);
    }

    #[test]
    fn a_worktree_add_waits_out_another_programs_half_written_worktree() {
        let (dir, git) = test_git("half-written");
        let root = dir.join("repo");
        assert!(git.init(&root).expect("git init").is_none());
        let repo = git.open(&root).expect("the repository");
        fs::write(root.join("a.txt"), "a\n").expect("a file");
        let base = repo.commit_changes("Base", true).expect("a commit");
        let specs = [
            crate::WorktreeSpec::Detached { at: base.clone() },
            crate::WorktreeSpec::NewBranch {
                name: "task-1".into(),
                start: base.clone(),
            },
        ];
        for (index, spec) in specs.into_iter().enumerate() {
            // Another program's `git worktree add` midway: its entry has no commondir yet.
            let entry = root.join(format!(".git/worktrees/other-{index}"));
            fs::create_dir_all(&entry).expect("the entry");
            let other = dir.join(format!("other-{index}"));
            fs::write(
                entry.join("gitdir"),
                format!("{}\n", other.join(".git").display()),
            )
            .expect("its gitdir");
            fs::write(entry.join("HEAD"), format!("{}\n", base.0)).expect("its HEAD");
            fs::write(entry.join("commondir"), "").expect("its commondir, not written yet");
            let finish = std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                fs::write(entry.join("commondir"), "../..\n").expect("its commondir");
            });
            let path = dir.join(format!("wt-{index}"));
            let worktree = repo
                .add_worktree(&path, spec)
                .expect("the worktree is added");
            finish.join().expect("no panic");
            assert_eq!(worktree.head().expect("its head"), base);
        }
        assert_eq!(repo.branch_tip("task-1").expect("the branch"), Some(base));
    }
}
