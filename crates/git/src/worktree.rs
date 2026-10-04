use crate::{
    Change, ChangeKind, CollisionKind, CommitOutcome, Error, Oid, PrepareOutcome, RebaseOutcome,
    Repo, Result, SeriesOutcome,
    command::{TempIndex, valid_oid, valid_path},
    parse,
    repo::TreeMerge,
};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// A Brigadier-created task/session checkout. Mutating operations require an idle worker.
#[derive(Debug)]
pub struct Worktree {
    repo: Repo,
    // Preparation stages the merged tree so previously tracked ignored additions stay visible.
    // Retain worker-origin provenance for the litter guard despite the engine's staging.
    prepared: Mutex<Option<Prepared>>,
}

#[derive(Debug)]
struct Prepared {
    head: Oid,
    known: BTreeSet<String>,
    untracked: BTreeSet<String>,
}

impl Worktree {
    pub(crate) fn new(repo: Repo) -> Self {
        Self {
            repo,
            prepared: Mutex::new(None),
        }
    }

    /// Canonical checkout path.
    pub fn path(&self) -> &Path {
        &self.repo.root
    }

    /// Current HEAD commit; an unborn checkout is an error.
    pub fn head(&self) -> Result<Oid> {
        self.repo.resolve("HEAD")
    }

    fn untracked(&self) -> Result<BTreeSet<String>> {
        let mut untracked = self.repo.status(false)?.untracked();
        if let Some(prepared) = &*self.prepared.lock().unwrap_or_else(|e| e.into_inner())
            && prepared.head == self.head()?
        {
            untracked.retain(|p| !prepared.known.contains(p) || prepared.untracked.contains(p));
            untracked.extend(prepared.untracked.iter().cloned());
        }
        Ok(untracked)
    }

    /// All net worker changes against base, including intermediate commits and untracked files.
    pub fn changes(&self, base: &Oid) -> Result<Vec<Change>> {
        valid_oid(base)?;
        let untracked = self.untracked()?;
        let (_, tree) = self.repo.capture()?;
        self.repo.tree_changes(base, &tree, &untracked)
    }

    /// Fold all worker content since base onto onto before inclusion selection and review.
    /// Conflicts are computed with merge-tree first and leave HEAD, index and files untouched.
    /// A snapshot base subtracts the user's original uncommitted content from the worker delta.
    pub fn prepare_candidate(&self, base: &Oid, onto: &Oid) -> Result<PrepareOutcome> {
        valid_oid(base)?;
        valid_oid(onto)?;
        self.ensure_idle()?;
        let original_head = self.head()?;
        let untracked = self.untracked()?;
        let (index, tree) = self.repo.capture()?;
        match self.repo.replay_tree(base, onto, &tree)? {
            TreeMerge::Conflicts(paths) => Ok(PrepareOutcome::Conflicts { paths }),
            TreeMerge::Ready(merged) => {
                let changes = self.repo.tree_changes(onto, &merged, &untracked)?;
                self.install(&index, &tree, &merged, &original_head, onto, &merged)?;
                *self.prepared.lock().unwrap_or_else(|e| e.into_inner()) = Some(Prepared {
                    head: onto.clone(),
                    known: changes.iter().map(|c| c.path.clone()).collect(),
                    untracked,
                });
                Ok(PrepareOutcome::Prepared { changes })
            }
        }
    }

    fn ensure_idle(&self) -> Result<()> {
        if let Some(what) = self.repo.busy()? {
            return Err(Error::Invalid(format!("worktree is busy: {what}")));
        }
        Ok(())
    }

    // Install only paths changed by the replay. read-tree's two-tree checkout protects against
    // edits since capture. Hold the real index lock across the checkout/ref/index transaction;
    // build both indexes first and use CAS for HEAD. Ignored files receive explicit protection.
    fn install(
        &self,
        index: &TempIndex,
        old_tree: &Oid,
        new_tree: &Oid,
        old_head: &Oid,
        new_head: &Oid,
        index_tree: &Oid,
    ) -> Result<()> {
        let changed = self.repo.changed_paths(old_tree, new_tree)?;
        for (path, kind) in self.repo.status(true)?.entries {
            if kind == CollisionKind::Ignored && changed.iter().any(|p| parse::overlaps(p, &path)) {
                return Err(Error::Invalid(format!(
                    "replay would overwrite ignored path {path:?}"
                )));
            }
        }
        let next_index = TempIndex::new()?;
        self.repo
            .index_cmd(&next_index, &["read-tree", &index_tree.0])?;
        let index_path = self.repo.git_path("index")?;
        let lock_path = index_path.with_file_name("index.lock");
        let mut lock_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)?;
        let lock = IndexLock::new(lock_path);
        lock_file.write_all(&fs::read(&next_index.path)?)?;
        lock_file.sync_all()?;
        drop(lock_file);
        if self.head()? != *old_head {
            return Err(Error::Invalid(
                "worker HEAD moved during preparation".into(),
            ));
        }
        self.repo
            .index_cmd(index, &["read-tree", "-m", "-u", &old_tree.0, &new_tree.0])?;
        let update = self.repo.cmd(
            &[
                "update-ref",
                "-m",
                "Brigadier candidate preparation",
                "HEAD",
                &new_head.0,
                &old_head.0,
            ],
            false,
        );
        if let Err(error) = update {
            self.repo
                .index_cmd(index, &["read-tree", "-m", "-u", &new_tree.0, &old_tree.0])?;
            return Err(error);
        }
        if let Err(error) = lock.publish(&index_path) {
            self.repo
                .cmd(&["update-ref", "HEAD", &old_head.0, &new_head.0], false)?;
            self.repo
                .index_cmd(index, &["read-tree", "-m", "-u", &new_tree.0, &old_tree.0])?;
            return Err(error.into());
        }
        Ok(())
    }

    /// Stage exactly the selected changes and run a normal commit, using repository hooks and
    /// git-config identity. Selecting a rename includes its source deletion. Exclusions remain
    /// uncommitted; hook failures return output and never claim a candidate was committed.
    pub fn commit_candidate(&self, include: &[String], message: &str) -> Result<CommitOutcome> {
        self.ensure_idle()?;
        let parent = self.head()?;
        let changes = self.changes(&parent)?;
        let mut paths = BTreeSet::new();
        for path in include {
            valid_path(path)?;
            let change = changes.iter().find(|c| &c.path == path).ok_or_else(|| {
                Error::Invalid(format!("included path is not a current change: {path:?}"))
            })?;
            paths.insert(path.clone());
            if let ChangeKind::Renamed { from } = &change.kind {
                paths.insert(from.clone());
            }
        }
        // Start with the target index, not the worker's previous staging choices.
        let index = TempIndex::new()?;
        self.repo.index_cmd(&index, &["read-tree", &parent.0])?;
        if !paths.is_empty() {
            let mut input = Vec::new();
            for path in &paths {
                input.extend_from_slice(path.as_bytes());
                input.push(0);
            }
            self.repo.git.checked(
                Some(self.path()),
                &[
                    "add",
                    "-A",
                    "--force",
                    "--pathspec-from-file=-",
                    "--pathspec-file-nul",
                ],
                false,
                &index.env(),
                Some(&input),
            )?;
        }
        let selected = parse::oid(&self.repo.index_cmd(&index, &["write-tree"])?)?;
        let empty = self.repo.changed_paths(&parent, &selected)?.is_empty();
        // Install the selected index before invoking normal git commit: hooks see the same index
        // and worktree as git, and may perform their normal formatting/staging behavior.
        let index_path = self.repo.git_path("index")?;
        let lock_path = index_path.with_file_name("index.lock");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)?;
        let lock = IndexLock::new(lock_path);
        file.write_all(&fs::read(&index.path)?)?;
        file.sync_all()?;
        drop(file);
        if self.head()? != parent {
            return Err(Error::Invalid("worker HEAD moved before commit".into()));
        }
        lock.publish(&index_path)?;
        if empty {
            return Ok(CommitOutcome::Empty);
        }
        let args = ["commit", "--file=-", "--cleanup=verbatim"];
        let out = self.repo.git.run(
            Some(self.path()),
            &args,
            false,
            &[],
            Some(message.as_bytes()),
        )?;
        let commit = self.head()?;
        if commit == parent {
            let mut output = String::from_utf8_lossy(&out.stdout).into_owned();
            output.push_str(&String::from_utf8_lossy(&out.stderr));
            if out.status.success() {
                return Err(Error::Invalid(
                    "git commit succeeded without advancing HEAD".into(),
                ));
            }
            return Ok(CommitOutcome::HookFailed {
                output: output.trim().to_owned(),
            });
        }
        // Post-commit hooks may fail after the commit was successfully created.
        let diff_stat = self.repo.diff_stat(&parent, &commit)?;
        *self.prepared.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(CommitOutcome::Committed { commit, diff_stat })
    }

    /// Replay a single candidate onto a newer target using a tree-only merge. Preserve excluded
    /// local files, use a CAS ref update, and report clean_fast only for disjoint changed paths.
    /// The caller must re-review overlapping successful replays before landing.
    pub fn rebase_candidate(
        &self,
        candidate: &Oid,
        old_onto: &Oid,
        new_onto: &Oid,
    ) -> Result<RebaseOutcome> {
        valid_oid(candidate)?;
        valid_oid(old_onto)?;
        valid_oid(new_onto)?;
        self.ensure_idle()?;
        if self.head()? != *candidate {
            return Err(Error::Invalid(
                "candidate is not this worktree's HEAD".into(),
            ));
        }
        let parents = self
            .repo
            .cmd(&["rev-list", "--parents", "-n", "1", &candidate.0], true)?;
        let parents = parse::line(&parents)?;
        let parents: Vec<_> = parents.split_whitespace().collect();
        if parents.len() != 2 || parents[1] != old_onto.0 {
            return Err(Error::Invalid(
                "candidate must have exactly old_onto as its parent".into(),
            ));
        }
        let paths = self.repo.changed_paths(old_onto, candidate)?;
        let target_paths = self.repo.changed_paths(old_onto, new_onto)?;
        let clean_fast = !paths
            .iter()
            .any(|p| target_paths.iter().any(|t| parse::overlaps(p, t)));
        match self.repo.replay_tree(old_onto, new_onto, candidate)? {
            TreeMerge::Conflicts(paths) => Ok(RebaseOutcome::Conflicts { paths }),
            TreeMerge::Ready(tree) => {
                let message = self
                    .repo
                    .cmd(&["show", "-s", "--format=%B", &candidate.0], true)?;
                let commit =
                    self.repo
                        .commit_tree(&tree, &[new_onto], parse::text(&message)?, false)?;
                // Local exclusions must not become part of the rebased candidate. Replay them
                // separately into the checkout's final content, keeping the commit tree exact.
                let (index, content) = self.repo.capture()?;
                match self.repo.replay_tree(candidate, &commit, &content)? {
                    TreeMerge::Conflicts(paths) => Ok(RebaseOutcome::Conflicts { paths }),
                    TreeMerge::Ready(working_tree) => {
                        self.install(&index, &content, &working_tree, candidate, &commit, &commit)?;
                        *self.prepared.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        Ok(RebaseOutcome::Rebased { commit, clean_fast })
                    }
                }
            }
        }
    }

    /// Replay the worker's commits `base..HEAD` (first-parent, oldest first) onto `onto` with
    /// tree-only merges, each as its change against its first parent. Changes to `exclude`
    /// (paths, covering everything under a folder) are dropped from every commit, so those
    /// paths keep the new parent's content; commits left empty are dropped. Messages and
    /// authors (name, email, date) are kept; hooks never run. The new tip is installed as HEAD
    /// with a CAS update, and uncommitted content (left-out files included) stays on disk.
    /// A conflict leaves HEAD, index and files untouched.
    pub fn replay_series(
        &self,
        base: &Oid,
        onto: &Oid,
        exclude: &[String],
    ) -> Result<SeriesOutcome> {
        valid_oid(base)?;
        valid_oid(onto)?;
        for path in exclude {
            valid_path(path)?;
        }
        self.ensure_idle()?;
        let head = self.head()?;
        if !self.repo.ancestor(base, &head)? {
            return Err(Error::Invalid("base is not an ancestor of HEAD".into()));
        }
        let range = format!("{}..{}", base.0, head.0);
        let list = self.repo.cmd(
            &[
                "rev-list",
                "--reverse",
                "--first-parent",
                "--parents",
                &range,
            ],
            true,
        )?;
        let covered = |path: &str| {
            exclude.iter().any(|ex| {
                let ex = ex.trim_end_matches('/');
                path == ex || path.strip_prefix(ex).is_some_and(|s| s.starts_with('/'))
            })
        };
        let mut tip = onto.clone();
        let mut commits = 0u32;
        let mut left_out = BTreeSet::new();
        for line in parse::text(&list)?.lines() {
            let mut ids = line.split_whitespace().map(|id| Oid(id.to_owned()));
            let (Some(commit), Some(parent)) = (ids.next(), ids.next()) else {
                return Err(Error::Invalid(
                    "the series holds a commit without a parent".into(),
                ));
            };
            let drop: Vec<String> = self
                .repo
                .changed_paths(&parent, &commit)?
                .into_iter()
                .filter(|p| covered(p))
                .collect();
            if drop.is_empty() && tip == parent {
                // Already on the new parent and nothing to leave out: keep the commit itself.
                tip = commit;
                commits += 1;
                continue;
            }
            let source = if drop.is_empty() {
                self.repo.tree_of(&commit.0)?
            } else {
                left_out.extend(drop.iter().cloned());
                let tree = self.repo.tree_reverting(&commit, &parent, &drop)?;
                if tree == self.repo.tree_of(&parent.0)? {
                    continue;
                }
                tree
            };
            match self.repo.replay_tree(&parent, &tip, &source)? {
                TreeMerge::Conflicts(paths) => return Ok(SeriesOutcome::Conflicts { paths }),
                TreeMerge::Ready(tree) => {
                    let (message, author) = self.authored(&commit)?;
                    tip = self
                        .repo
                        .commit_tree_with(&tree, &[&tip], &message, &author)?;
                    commits += 1;
                }
            }
        }
        if tip == head {
            return Ok(SeriesOutcome::Replayed {
                tip,
                commits,
                rewritten: false,
            });
        }
        // Replay the checkout's content from a HEAD whose left-out paths already match the new
        // tip, so the series' left-out files stay on disk as uncommitted content.
        let from = if left_out.is_empty() {
            head.clone()
        } else {
            let paths: Vec<String> = left_out.into_iter().collect();
            let tree = self.repo.tree_reverting(&head, &tip, &paths)?;
            self.repo.commit_tree(
                &tree,
                &[&head],
                "Brigadier series without left-out paths",
                true,
            )?
        };
        let (index, content) = self.repo.capture()?;
        match self.repo.replay_tree(&from, &tip, &content)? {
            TreeMerge::Conflicts(paths) => Ok(SeriesOutcome::Conflicts { paths }),
            TreeMerge::Ready(working_tree) => {
                self.install(&index, &content, &working_tree, &head, &tip, &tip)?;
                *self.prepared.lock().unwrap_or_else(|e| e.into_inner()) = None;
                Ok(SeriesOutcome::Replayed {
                    tip,
                    commits,
                    rewritten: true,
                })
            }
        }
    }

    /// A commit's exact message and its author's name, email and date as git environment.
    fn authored(&self, commit: &Oid) -> Result<(String, Vec<(OsString, OsString)>)> {
        let raw = self.repo.cmd(&["cat-file", "commit", &commit.0], true)?;
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

    /// Preserve all remaining tracked/untracked non-ignored work as one WIP commit. Plumbing
    /// intentionally avoids validation hooks so unfinished work can survive worktree removal.
    /// Identity comes from git config. Nothing is created for a clean checkout.
    pub fn commit_wip(&self, message: &str) -> Result<Option<Oid>> {
        self.ensure_idle()?;
        if self.repo.symbolic_head()?.is_none() {
            return Err(Error::Invalid(
                "WIP requires a branch so it survives worktree removal".into(),
            ));
        }
        if self.repo.status(false)?.dirty().is_empty() {
            return Ok(None);
        }
        let head = self.head()?;
        let (index, tree) = self.repo.capture()?;
        if self.repo.changed_paths(&head, &tree)?.is_empty() {
            return Ok(None);
        }
        let commit = self.repo.commit_tree(&tree, &[&head], message, false)?;
        self.install(&index, &tree, &tree, &head, &commit, &commit)?;
        *self.prepared.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Ok(Some(commit))
    }

    /// Kept work must not carry the user's uncommitted changes the worker started from: replace
    /// the branch's commits since `snapshot` with one commit of the same changes on the
    /// snapshot's parent (the user's HEAD). Returns the new tip, or the conflicting paths when
    /// the work overlaps the uncommitted changes (the branch is then left as it is).
    pub fn drop_snapshot(
        &self,
        snapshot: &Oid,
        message: &str,
    ) -> Result<std::result::Result<Oid, Vec<String>>> {
        valid_oid(snapshot)?;
        self.ensure_idle()?;
        let branch = self
            .repo
            .symbolic_head()?
            .ok_or_else(|| Error::Invalid("kept work needs a branch".into()))?;
        let head = self.head()?;
        let parent = self.repo.resolve(&format!("{}^", snapshot.0))?;
        let tree = parse::oid(&self.repo.cmd(
            &["rev-parse", "--verify", &format!("{}^{{tree}}", head.0)],
            true,
        )?)?;
        match self.repo.replay_tree(snapshot, &parent, &tree)? {
            TreeMerge::Conflicts(paths) => Ok(Err(paths)),
            TreeMerge::Ready(tree) => {
                let commit = self.repo.commit_tree(&tree, &[&parent], message, false)?;
                let name = format!("refs/heads/{branch}");
                self.repo.cmd(
                    &[
                        "update-ref",
                        "-m",
                        "Brigadier kept work without the uncommitted snapshot",
                        &name,
                        &commit.0,
                        &head.0,
                    ],
                    false,
                )?;
                Ok(Ok(commit))
            }
        }
    }

    /// Full final-content patch against base, including non-ignored untracked files. A
    /// checkout stuck in a conflicted merge or rebase is read as its files stand (conflict
    /// markers included), so a worker's unfinished work there can still be kept.
    pub fn diff_from(&self, base: &Oid) -> Result<String> {
        valid_oid(base)?;
        let tree = match self.repo.capture() {
            Ok((_, tree)) => tree,
            Err(_) if self.repo.status(false)?.unmerged => self.repo.files_tree()?,
            Err(err) => return Err(err),
        };
        self.repo.diff(base, &tree)
    }
}

struct IndexLock {
    path: PathBuf,
    held: bool,
}
impl IndexLock {
    fn new(path: PathBuf) -> Self {
        Self { path, held: true }
    }
    fn publish(mut self, index: &Path) -> std::io::Result<()> {
        fs::rename(&self.path, index)?;
        // The lock name is available again: never remove a new lock another git command
        // acquired after our rename (including the upcoming git commit's own lock).
        self.held = false;
        Ok(())
    }
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        if self.held {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Git, LandOutcome, LandRequest, Oid, Repo, SeriesOutcome, Worktree, WorktreeSpec};
    use std::{ffi::OsString, fs, path::PathBuf};

    const AUTHOR: &str = "--author=Worker <worker@example.com>";
    const DATE: &str = "--date=2001-02-03T04:05:06+0100";

    struct Fixture {
        dir: PathBuf,
        repo: Repo,
        main: String,
        base: Oid,
        wt: Worktree,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(name: &str) -> Fixture {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "brigadier-git-series-{name}-{}-{nanos}",
            std::process::id()
        ));
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
        fs::write(root.join("shared.txt"), "one\n").expect("a file");
        let base = repo.commit_changes("Base", true).expect("a commit");
        let main = repo.symbolic_head().expect("HEAD").expect("a branch");
        let wt = repo
            .add_worktree(
                &dir.join("wt"),
                WorktreeSpec::NewBranch {
                    name: "task".into(),
                    start: base.clone(),
                },
            )
            .expect("a worktree");
        Fixture {
            dir,
            repo,
            main,
            base,
            wt,
        }
    }

    /// Write `files` and commit everything as the worker, with a fixed author and date.
    fn commit(repo: &Repo, message: &str, files: &[(&str, &str)]) -> Oid {
        for (path, content) in files {
            let path = repo.root().join(path);
            fs::create_dir_all(path.parent().expect("a parent")).expect("a folder");
            fs::write(path, content).expect("a file");
        }
        repo.cmd(&["add", "-A", "--", "."], false).expect("add");
        repo.cmd(&["commit", "--quiet", AUTHOR, DATE, "-m", message], false)
            .expect("a commit");
        repo.resolve("HEAD").expect("HEAD")
    }

    fn text(repo: &Repo, args: &[&str]) -> String {
        String::from_utf8(repo.cmd(args, true).expect("git output")).expect("UTF-8")
    }

    fn paths(repo: &Repo, commit: &Oid) -> Vec<String> {
        text(repo, &["ls-tree", "-r", "--name-only", &commit.0])
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Message, author and author date of each commit in `from..to`, oldest first.
    fn series(repo: &Repo, from: &Oid, to: &Oid) -> Vec<String> {
        let range = format!("{}..{}", from.0, to.0);
        text(
            repo,
            &[
                "log",
                "--reverse",
                "--format=%s|%an <%ae> %ad",
                "--date=raw",
                &range,
            ],
        )
        .lines()
        .map(str::to_owned)
        .collect()
    }

    fn replayed(outcome: SeriesOutcome) -> (Oid, u32, bool) {
        match outcome {
            SeriesOutcome::Replayed {
                tip,
                commits,
                rewritten,
            } => (tip, commits, rewritten),
            SeriesOutcome::Conflicts { paths } => panic!("unexpected conflicts: {paths:?}"),
        }
    }

    #[test]
    fn a_series_already_on_its_target_is_left_alone() {
        let f = fixture("fast");
        commit(&f.wt.repo, "Add a", &[("a.rs", "a\n")]);
        let head = commit(&f.wt.repo, "Add b", &[("b.rs", "b\n")]);
        let (tip, commits, rewritten) =
            replayed(f.wt.replay_series(&f.base, &f.base, &[]).expect("replay"));
        assert_eq!((tip, commits, rewritten), (head.clone(), 2, false));
        assert_eq!(f.wt.head().expect("HEAD"), head);
    }

    #[test]
    fn committed_litter_is_left_out_of_every_commit_but_stays_on_disk() {
        let f = fixture("litter");
        let repo = &f.wt.repo;
        let first = commit(repo, "Add a", &[("a.rs", "a\n")]);
        let head = commit(
            repo,
            "Add b\n\nWith a body.\n",
            &[("b.rs", "b\n"), ("debug.log", "noise\n")],
        );
        let before = series(repo, &f.base, &head);
        fs::write(repo.root().join("a.rs"), "a, edited\n").expect("an uncommitted edit");
        let outcome = f.wt.replay_series(&f.base, &f.base, &["debug.log".into()]);
        let (tip, commits, rewritten) = replayed(outcome.expect("replay"));
        assert_eq!((commits, rewritten), (2, true));
        assert_eq!(f.wt.head().expect("HEAD"), tip);
        let commits: Vec<Oid> = text(
            repo,
            &["rev-list", "--reverse", &format!("{}..{}", f.base.0, tip.0)],
        )
        .lines()
        .map(|id| Oid(id.to_owned()))
        .collect();
        assert_eq!(commits[0], first, "an untouched prefix keeps its commits");
        for commit in &commits {
            assert!(!paths(repo, commit).contains(&"debug.log".to_owned()));
        }
        assert_eq!(paths(repo, &tip), ["a.rs", "b.rs", "shared.txt"]);
        assert_eq!(series(repo, &f.base, &tip), before);
        assert_eq!(
            text(repo, &["log", "-1", "--format=%B", &tip.0]),
            "Add b\n\nWith a body.\n\n"
        );
        let root = repo.root();
        assert_eq!(
            fs::read_to_string(root.join("debug.log")).expect("kept"),
            "noise\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("a.rs")).expect("kept"),
            "a, edited\n"
        );
        let status = text(repo, &["status", "--porcelain", "--untracked-files=all"]);
        assert_eq!(status, " M a.rs\n?? debug.log\n");
    }

    #[test]
    fn a_series_moves_onto_a_target_that_moved_and_lands() {
        let f = fixture("moved");
        fs::write(f.repo.root().join("other.txt"), "target\n").expect("a file");
        let onto = f
            .repo
            .commit_changes("Target work", true)
            .expect("a commit");
        let head = commit(&f.wt.repo, "Add a", &[("a.rs", "a\n")]);
        let before = series(&f.wt.repo, &f.base, &head);
        let (tip, commits, rewritten) =
            replayed(f.wt.replay_series(&f.base, &onto, &[]).expect("replay"));
        assert_eq!((commits, rewritten), (1, true));
        assert!(f.repo.ancestor(&onto, &tip).expect("ancestry"));
        assert_eq!(paths(&f.repo, &tip), ["a.rs", "other.txt", "shared.txt"]);
        assert_eq!(series(&f.repo, &onto, &tip), before);
        assert!(f.wt.path().join("other.txt").exists());
        let landed = f
            .repo
            .land(&LandRequest {
                branch: f.main.clone(),
                expected_tip: onto,
                commit: tip.clone(),
            })
            .expect("land");
        assert!(matches!(landed, LandOutcome::Landed { new_tip } if new_tip == tip));
    }

    #[test]
    fn a_conflict_with_the_target_changes_nothing() {
        let f = fixture("conflict");
        fs::write(f.repo.root().join("shared.txt"), "target\n").expect("a file");
        let onto = f
            .repo
            .commit_changes("Target edit", true)
            .expect("a commit");
        let repo = &f.wt.repo;
        let head = commit(repo, "Worker edit", &[("shared.txt", "worker\n")]);
        fs::write(repo.root().join("notes.txt"), "notes\n").expect("an untracked file");
        let status = text(repo, &["status", "--porcelain", "--untracked-files=all"]);
        let index = text(repo, &["write-tree"]);
        match f.wt.replay_series(&f.base, &onto, &[]).expect("replay") {
            SeriesOutcome::Conflicts { paths } => assert_eq!(paths, ["shared.txt"]),
            other => panic!("expected conflicts, got {other:?}"),
        }
        assert_eq!(f.wt.head().expect("HEAD"), head);
        assert_eq!(
            text(repo, &["status", "--porcelain", "--untracked-files=all"]),
            status
        );
        assert_eq!(text(repo, &["write-tree"]), index);
        let shared = fs::read_to_string(repo.root().join("shared.txt")).expect("a file");
        assert_eq!(shared, "worker\n");
    }

    #[test]
    fn a_commit_of_only_litter_is_dropped() {
        let f = fixture("dropped");
        let repo = &f.wt.repo;
        commit(repo, "Add a", &[("a.rs", "a\n")]);
        commit(
            repo,
            "Add logs",
            &[("debug.log", "noise\n"), ("logs/run.log", "run\n")],
        );
        let head = commit(repo, "Add b", &[("b.rs", "b\n")]);
        let exclude = ["debug.log".to_owned(), "logs".to_owned()];
        let (tip, commits, rewritten) = replayed(
            f.wt.replay_series(&f.base, &f.base, &exclude)
                .expect("replay"),
        );
        assert_eq!((commits, rewritten), (2, true));
        assert_ne!(tip, head);
        let subjects = text(
            repo,
            &[
                "log",
                "--reverse",
                "--format=%s",
                &format!("{}..{}", f.base.0, tip.0),
            ],
        );
        assert_eq!(subjects, "Add a\nAdd b\n");
        assert_eq!(paths(repo, &tip), ["a.rs", "b.rs", "shared.txt"]);
        assert!(repo.root().join("debug.log").exists());
        assert!(repo.root().join("logs/run.log").exists());
    }
}
