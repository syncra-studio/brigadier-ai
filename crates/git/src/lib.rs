//! Synchronous git CLI engine for isolated worker worktrees and reviewed fast-forward landings.
//!
//! Call on a blocking thread. Git 2.38 or later is required; the caller supplies the binary and
//! login environment. Temporary snapshots never update the user's index, stash or refs. Paths
//! exposed as strings must be UTF-8; unsupported names fail explicitly rather than being aliased.
#![warn(missing_docs)]

mod command;
pub mod litter;
mod parse;
mod repo;
mod source;
mod trailers;
mod worktree;

use std::{ffi::OsString, path::PathBuf};

pub use command::{FoundRepo, Git};
pub use repo::Repo;
pub use trailers::strip_ai_coauthors;
pub use worktree::Worktree;

/// A git operation's result.
pub type Result<T> = std::result::Result<T, Error>;

/// A command, repository, input or output failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Git is older than the required 2.38.
    #[error("git {found} is too old; Brigadier requires git 2.38 or later")]
    TooOld {
        /// Reported version.
        found: String,
    },
    /// The supplied folder is not the top level of a working repository.
    #[error("not a repository top level: {0}")]
    NotARepository(PathBuf),
    /// A git command failed; stderr is trimmed but otherwise unmodified.
    #[error("git {args} failed ({code:?}): {stderr}")]
    Command {
        /// Arguments, for diagnostics only.
        args: String,
        /// Process exit status, absent if terminated by a signal.
        code: Option<i32>,
        /// Git's error output.
        stderr: String,
    },
    /// Filesystem or process I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Git produced output that cannot be interpreted safely.
    #[error("invalid git output: {0}")]
    Parse(String),
    /// The operation is invalid in the current state.
    #[error("{0}")]
    Invalid(String),
}

/// A 40/64-hex object id; operations validate externally constructed values.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Oid(pub String);

/// A local branch and the worktree, if any, holding it.
#[derive(Debug, Clone, PartialEq)]
pub struct Branch {
    /// Branch name without refs/heads/.
    pub name: String,
    /// Current commit.
    pub commit: Oid,
    /// Checkout holding this branch.
    pub checked_out_at: Option<PathBuf>,
}

/// Current state of the user's checkout.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoState {
    /// None for detached HEAD.
    pub current_branch: Option<String>,
    /// None for an unborn branch.
    pub head: Option<Oid>,
    /// Local branches sorted by name.
    pub branches: Vec<Branch>,
    /// Staged, unstaged and untracked non-ignored repo-relative paths.
    pub dirty_files: Vec<String>,
}

/// A registered checkout from git's worktree metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeInfo {
    /// Absolute worktree path.
    pub path: PathBuf,
    /// None for an unborn checkout.
    pub head: Option<Oid>,
    /// None for detached HEAD.
    pub branch: Option<String>,
    /// Git protects this worktree against pruning/removal.
    pub locked: bool,
    /// Git considers this worktree's metadata stale.
    pub prunable: bool,
}

/// What a new worktree checks out.
#[derive(Debug, Clone)]
pub enum WorktreeSpec {
    /// Create a new branch; fail if it already exists.
    NewBranch {
        /// New branch name.
        name: String,
        /// Starting commit.
        start: Oid,
    },
    /// Check out an existing branch; fail if held elsewhere.
    Branch {
        /// Existing local branch name.
        name: String,
    },
    /// Check out a commit without attaching a branch.
    Detached {
        /// Starting commit.
        at: Oid,
    },
}

/// Uncommitted content captured without changing the checkout, index, stash or refs.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Unreferenced snapshot commit; the caller should keep its id for the session lifetime.
    pub commit: Oid,
    /// Original HEAD, used as the snapshot's parent.
    pub head: Oid,
    /// Captured dirty paths.
    pub files: Vec<String>,
}

/// A reviewed candidate and the target tip it was built against.
#[derive(Debug, Clone)]
pub struct LandRequest {
    /// Destination local branch.
    pub branch: String,
    /// Landing is refused if this tip changed.
    pub expected_tip: Oid,
    /// Candidate, which must descend from expected_tip.
    pub commit: Oid,
}

/// Result of a guarded landing.
#[derive(Debug, Clone)]
pub enum LandOutcome {
    /// The branch advanced successfully.
    Landed {
        /// New branch tip.
        new_tip: Oid,
    },
    /// No landing was performed.
    Blocked(LandBlock),
}

/// Why a landing could not proceed; Display supplies a human-readable sentence.
#[derive(Debug, Clone, thiserror::Error)]
pub enum LandBlock {
    /// The destination changed after the candidate was built.
    #[error("The target branch moved to {}.", actual.0)]
    TipMoved {
        /// Current destination tip.
        actual: Oid,
    },
    /// The discovered checkout switched branches before mutation.
    #[error("The checkout at {} switched branches (it is now {}).", worktree.display(), match now {
        Some(branch) => format!("on `{branch}`"),
        None => "on a detached HEAD".to_owned(),
    })]
    BranchSwitched {
        /// Checkout that switched.
        worktree: PathBuf,
        /// Its current branch, or None for detached HEAD.
        now: Option<String>,
    },
    /// Landing would overwrite local changes or files.
    #[error("Landing would overwrite local files in {}: {}.", worktree.display(), describe(paths))]
    Collisions {
        /// Checkout with collisions.
        worktree: PathBuf,
        /// Affected local paths and their origin.
        paths: Vec<CollidingPath>,
    },
    /// An operation/lock is active, or git refused the preflighted fast-forward.
    #[error("The checkout at {} is busy: {what}.", worktree.display())]
    CheckoutBusy {
        /// Affected checkout.
        worktree: PathBuf,
        /// Operation, lock or git refusal diagnostic.
        what: String,
    },
    /// The candidate does not descend from the expected target.
    #[error("The candidate is not a fast-forward of the target branch.")]
    NotFastForward,
}

/// `a.txt (modified), docs/USAGE.md (ignored)`.
fn describe(paths: &[CollidingPath]) -> String {
    paths
        .iter()
        .map(|p| {
            let kind = match p.kind {
                CollisionKind::Modified => "modified",
                CollisionKind::Untracked => "untracked",
                CollisionKind::Ignored => "ignored",
            };
            format!("{} ({kind})", p.path)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A path that would be overwritten by a landing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollidingPath {
    /// Repo-relative local path.
    pub path: String,
    /// How the local file differs from HEAD.
    pub kind: CollisionKind,
}

/// Origin of a landing collision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollisionKind {
    /// Tracked staged or unstaged modification.
    Modified,
    /// Untracked non-ignored content.
    Untracked,
    /// Ignored local content.
    Ignored,
}

/// Preparing a session branch for landing never modifies a checkout.
#[derive(Debug, Clone)]
pub enum MergeOutcome {
    /// A commit ready for a separate guarded landing.
    Ready {
        /// Fast-forward tip or generated merge commit.
        commit: Oid,
        /// Base branch tip at preparation time.
        base_tip: Oid,
        /// No new merge commit was required.
        fast_forward: bool,
    },
    /// The merge needs resolution.
    Conflicts {
        /// Conflicted paths (possibly empty for a tree-level conflict).
        paths: Vec<String>,
    },
}

/// A branch merged into a task's work for a merge worker to finish ([`Repo::merge_for_resolution`]).
#[derive(Debug, Clone)]
pub struct MergeStart {
    /// The two-parent commit holding the merged tree, conflict markers left in place.
    pub commit: Oid,
    /// Its first parent: the task's work, without the files left out.
    pub work: Oid,
    /// Its second parent: the branch tip merged in.
    pub onto: Oid,
    /// The merge had no conflict at all.
    pub clean: bool,
    /// Conflicted paths. Text conflicts carry markers; binary, mode, modify/delete and
    /// file/directory conflicts don't. Empty for a conflict git can't pin to a path.
    pub conflicts: Vec<String>,
}

/// A final content change relative to a base commit.
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeKind {
    /// New path.
    Added,
    /// Changed contents or executable bit.
    Modified,
    /// Removed path.
    Deleted,
    /// A rename detected by git.
    Renamed {
        /// Original repo-relative path.
        from: String,
    },
    /// Changed object type (for example file to symlink).
    TypeChanged,
}

/// A worker change, including previously committed and currently untracked work.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// Repo-relative destination path.
    pub path: String,
    /// Content change type.
    pub kind: ChangeKind,
    /// Never staged or committed in the worker's worktree.
    pub untracked: bool,
}

/// Result of folding worker work onto the current target tip.
#[derive(Debug, Clone)]
pub enum PrepareOutcome {
    /// Worktree HEAD is onto, with the worker's net changes uncommitted.
    Prepared {
        /// Sorted changes for the litter guard and inclusion selection.
        changes: Vec<Change>,
    },
    /// No checkout, index or ref was changed.
    Conflicts {
        /// Conflicted paths.
        paths: Vec<String>,
    },
}

/// Result of a normal git commit with repository hooks enabled.
#[derive(Debug, Clone)]
pub enum CommitOutcome {
    /// A candidate was committed.
    Committed {
        /// New single-parent candidate commit.
        commit: Oid,
        /// Content statistics relative to its parent.
        diff_stat: DiffStat,
    },
    /// Commit was refused; HEAD did not advance (includes hook diagnostics).
    HookFailed {
        /// Git and hook output.
        output: String,
    },
    /// No included content differs from HEAD.
    Empty,
}

/// Building a branch from a saved patch never modifies a checkout.
#[derive(Debug, Clone)]
pub enum PatchOutcome {
    /// The branch was created at one new commit of the patch.
    Applied {
        /// The new commit.
        commit: Oid,
    },
    /// The patch conflicts with the start commit; nothing was created.
    Conflicts {
        /// Conflicted paths.
        paths: Vec<String>,
    },
    /// The patch does not apply at all (git's reason); nothing was created.
    Failed {
        /// Git's message.
        reason: String,
    },
}

/// Result of replaying a candidate onto a target that moved.
#[derive(Debug, Clone)]
pub enum RebaseOutcome {
    /// The candidate has a new target parent.
    Rebased {
        /// Rewritten commit.
        commit: Oid,
        /// Target changes and candidate paths do not overlap; no new review is needed.
        clean_fast: bool,
    },
    /// Nothing was changed in the checkout.
    Conflicts {
        /// Conflicted paths.
        paths: Vec<String>,
    },
}

/// Result of replaying a worker's commit series onto a target tip.
#[derive(Debug, Clone)]
pub enum SeriesOutcome {
    /// The worktree's HEAD is now `tip`; its branch holds the series on `onto`.
    Replayed {
        /// New HEAD: the last replayed commit, or `onto` when no commit was left.
        tip: Oid,
        /// Commits in the resulting series, after dropping ones left empty.
        commits: u32,
        /// HEAD moved; false when the series already sat on `onto` without left-out paths.
        rewritten: bool,
    },
    /// Nothing was changed in the checkout.
    Conflicts {
        /// Conflicted paths of the first commit that did not apply.
        paths: Vec<String>,
    },
}

/// Result of preparing a commit that reverts landed commits (the user's Undo).
#[derive(Debug, Clone)]
pub enum RevertOutcome {
    /// An unreferenced commit on the given tip; `land` puts it on the branch.
    Ready {
        /// The revert commit.
        commit: Oid,
    },
    /// A later commit changed the same paths, so nothing was prepared.
    Touched {
        /// The paths changed since.
        paths: Vec<String>,
    },
    /// The revert does not apply cleanly (a safety net: `Touched` normally comes first).
    Conflicts {
        /// Conflicted paths.
        paths: Vec<String>,
    },
}

/// A checkout as it stands, for a review of its uncommitted changes. Nothing was written to
/// its index or files to get it.
#[derive(Debug, Clone)]
pub struct CheckoutTrees {
    /// The checked-out commit; absent in an unborn repository.
    pub head: Option<Oid>,
    /// The tree of HEAD, or the empty tree in an unborn repository.
    pub head_tree: Oid,
    /// The tree of the index (what is staged).
    pub staged: Oid,
    /// The tree of the files (HEAD plus every non-ignored change).
    pub files: Oid,
    /// Files git does not track yet.
    pub untracked: Vec<String>,
}

/// How a file differs on one side of a checkout: its index from HEAD, or its files from the
/// index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Changed contents or executable bit.
    Modified,
    /// New path.
    Added,
    /// Removed path.
    Deleted,
    /// Moved from another path.
    Renamed,
    /// Copied from another path.
    Copied,
    /// Changed object type (for example file to symlink).
    TypeChanged,
    /// Not tracked by git (never on the staged side).
    Untracked,
    /// Unmerged index entries (never on the staged side).
    Conflicted,
}

/// One file of a checkout's source control list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceEntry {
    /// Repo-relative path.
    pub path: String,
    /// The path a rename or copy came from.
    pub old_path: Option<String>,
    /// How it differs.
    pub kind: SourceKind,
}

/// A checkout's files split as `git status` sees them, each list sorted by path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SourceChanges {
    /// The index against HEAD.
    pub staged: Vec<SourceEntry>,
    /// The files against the index, untracked and conflicted files included. A partly staged
    /// file is in both lists.
    pub changes: Vec<SourceEntry>,
}

/// Where a branch pushes, and how far it is ahead.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteState {
    /// The remote it pushes to: its upstream's, else `origin` when there is one.
    pub remote: Option<String>,
    /// Its upstream (`origin/main`), once it has one.
    pub upstream: Option<String>,
    /// Commits not pushed yet: ahead of its upstream, or on no remote branch at all.
    pub ahead: u32,
}

/// One commit of a branch's history.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitInfo {
    /// Its id.
    pub commit: Oid,
    /// The first line of its message.
    pub subject: String,
    /// When it was committed, in ms since the Unix epoch.
    pub at_ms: i64,
}

/// Per-path and aggregate line statistics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiffStat {
    /// Sorted paths, with binary files included.
    pub files: Vec<FileStat>,
    /// Total added text lines.
    pub insertions: u32,
    /// Total deleted text lines.
    pub deletions: u32,
}

/// Diff statistics for one path (renames use the destination).
#[derive(Debug, Clone, PartialEq)]
pub struct FileStat {
    /// Repo-relative path.
    pub path: String,
    /// Added lines, zero for binary files.
    pub insertions: u32,
    /// Deleted lines, zero for binary files.
    pub deletions: u32,
    /// Git classified the content as binary.
    pub binary: bool,
}

// Keep OsString in the crate's public API documentation close to Git's environment contract.
type Environment = Vec<(OsString, OsString)>;
