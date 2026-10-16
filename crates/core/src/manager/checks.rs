//! `run_check` (THREAD-PLAN.md Q8 lever 3): the checks workers and the thread run (tests,
//! lint, typecheck, build), each result kept per tree, so a check never runs twice on the same
//! files.
//!
//! - **Where and how.** The command runs in the caller's checkout (a worker's worktree, the
//!   thread's workspace), `workdir` relative to it, at the caller's access and recorded under
//!   its cleanup owner, as `run` runs: at Full access the plain shell, else a Codex caller's
//!   own sandbox (`codex sandbox`) and a Claude caller's Seatbelt profile
//!   ([`SessionManager::seatbelt_spec`]); a read-only Codex worker's from its scratch folder,
//!   as its own shell runs, so its checks can't write its checkout either. Its process is in the cleanup ledger while it runs,
//!   which is where the machine guard looks for builds: a heavy check (`cargo test`, `pnpm
//!   build`) takes the build lease and waits for it like a worker's own (`crate::machine`).
//! - **The key.** The git tree of the checkout as it stands (uncommitted and untracked, not
//!   ignored files included; a private copy of the index, never the real one), the command,
//!   the workdir relative to the repository's root, and what git doesn't see: the project's
//!   secret files; the ignored `.env*` files, lockfiles and package manager and Cargo
//!   configuration of the root, the workdir and every package folder; that configuration in
//!   the user's home; and the toolchains' versions. When any part can't be told, the
//!   cache is left out: the command runs and its result is not kept (a stale pass is worse
//!   than a run). One run of a key at a time: a caller that waited answers from the cache.
//! - **The value.** A finished run's status, the reply the model got and its whole output
//!   (stored as the conversation's `out-…` output, so `read_artifact` reads it), the
//!   project's secrets hidden as in the rest of the caller's output. Failures are kept too:
//!   the same files fail again, under the same sandbox (another sandbox runs it again). A
//!   worker gets its own copy of a long output in its scratch folder. `rerun` runs and
//!   replaces.
//! - **Affected checks.** With no command, `run_check` answers with the checks the changes
//!   since the caller's base call for: those of each changed package and of the packages that
//!   depend on it, everything for a change to the workspace's own manifests and configuration
//!   or to a path no package holds. A check that passes in a package folder is learned into
//!   the project's Brain (`checks:<folder>`) and offered for it from then on.
//!
//! Each run and each answer from the cache is recorded ([`DomainEvent::CheckRan`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use brigadier_brain::{Claim, NewNode, NodeKind, Origin, Provenance};
use brigadier_providers::redact::Redactor;
use brigadier_providers::{Access, ProviderKind};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::SessionManager;
use super::run::{
    RUN_TIMEOUT_DEFAULT, RUN_TIMEOUT_MAX, run_command_then, run_workdir, shell_command,
};
use super::workers::ToolAccess;
use super::{blocking, git_error};
use crate::digest::TRIM_ABOVE;
use crate::model::{ConversationId, DomainEvent, Environment, ProjectId, Setup};
use crate::tools::RunCheck;
use crate::work::{OutputSource, TaskId};
use crate::{Error, Result, now_ms};

/// A heavy check's hold on the build lease: given back when the check ends or won't start.
struct CheckLease {
    machine: Arc<crate::machine::MachineWatch>,
    ticket: u64,
}

impl Drop for CheckLease {
    fn drop(&mut self) {
        self.machine.end_check(self.ticket);
    }
}

/// How long a toolchain's version is trusted before it is asked again.
const TOOLCHAIN_TTL: Duration = Duration::from_secs(5 * 60);
/// How long a version command may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);
/// The package managers' and Cargo's own configuration, in a folder or the user's home: a
/// check loads it whether git sees it or not.
const CONFIG_FILES: &[&str] = &[
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".cargo/config.toml",
    ".cargo/config",
];
/// Files that pin a build's dependencies.
const LOCKFILES: &[&str] = &[
    "Cargo.lock",
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
];
/// The toolchains in a check's key: each one's name, the command that tells its version, and
/// the words of a command that use it.
const TOOLCHAINS: &[(&str, &str, &[&str])] = &[
    (
        "rustc",
        "rustc -V",
        &["cargo", "rustc", "rustup", "rustfmt", "clippy-driver"],
    ),
    ("cargo", "cargo -V", &["cargo"]),
    (
        "node",
        "node --version",
        &[
            "node", "npm", "npx", "pnpm", "yarn", "tsc", "vitest", "jest", "eslint", "prettier",
        ],
    ),
    ("pnpm", "pnpm --version", &["pnpm"]),
];
/// The `package.json` scripts offered as checks.
const SCRIPTS: &[&str] = &["test", "lint", "typecheck", "check", "build", "fmt"];
/// A version of the key's recipe: results kept under another recipe are never found.
/// Recipe 2: a thread's and the full checks' `TMPDIR` moved out of the data folder, so a socket
/// bind that failed there never answers from the cache.
const KEY_RECIPE: &str = "brigadier-check-2";

/// Who calls `run_check`, and where its commands run.
struct Caller {
    conversation_id: ConversationId,
    task_id: Option<TaskId>,
    provider: ProviderKind,
    access: Access,
    /// Cancelled when its CLI (or terminal) ends.
    ended: CancellationToken,
    /// The cleanup-ledger owner its commands are recorded under.
    owner: String,
    /// The checkout: a worker's worktree, the thread's workspace.
    tree: Option<PathBuf>,
    scratch: PathBuf,
    /// Its commands' `TMPDIR`: a task's test data folder, the thread's or the full checks'
    /// short temporary folder ([`super::workers::session_temp_dir`]), never a folder in the
    /// app's data folder, whose long path with spaces breaks a test that binds a socket there.
    tmp: PathBuf,
    /// The folder its access's writable working directory is, when that isn't its checkout
    /// (a read-only Codex worker works from its scratch folder): its checks run from there
    /// too, so they can't write a checkout its own shell can't.
    cwd_grant: Option<PathBuf>,
    /// What hides its project's secrets (and the session's grants) in a check's output, as
    /// in the rest of its output.
    redactor: Option<Arc<Redactor>>,
    /// What the affected checks compare the checkout against.
    base: Base,
    project: Option<ProjectId>,
    /// The project's secret files, repository-relative.
    secret_files: Vec<String>,
    /// Its commands run at low OS priority: a worker's, and the full checks.
    low_priority: bool,
}

/// What `run_check`'s `commit` says on the user's own checkout.
const NOT_ON_LOCAL: &str = "Not committed: this is the user's own checkout, where you commit only when they ask, with git yourself.";

/// Longest a `run_check` commit may take, the repository's hooks included.
const COMMIT_TIMEOUT: Duration = Duration::from_secs(if cfg!(test) { 5 } else { 300 });

/// The shell lines of `run_check`'s `commit` (BUILD-PLAN C9): everything changed in the
/// checkout staged and committed with `message` and the thread's trailer. They run as the
/// check did, at its access and in its environment, since git runs the repository's hooks and
/// filters. Exit 100: nothing to commit; 101: `git add` failed.
fn commit_command(message: &str) -> String {
    let quoted = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    format!(
        "git add -A || exit 101\ngit diff --cached --quiet && exit 100\ngit commit -q -m {} --trailer {}",
        quoted(message),
        quoted(super::prompts::THREAD_TRAILER)
    )
}

/// What the commit `commit_command` made in `tree` holds, and what it left (BUILD-PLAN C10
/// e), in one line, so the thread needs no git call of its own to say so.
fn commit_record(git: &brigadier_git::Git, tree: &Path, message: &str) -> String {
    let subject = message.lines().next().unwrap_or_default();
    let repo = match git.open(tree) {
        Ok(repo) => repo,
        Err(err) => {
            return format!("Committed \"{subject}\", but git could not read it back: {err}");
        }
    };
    let read = |args: &[&str]| repo.read_only(args).ok();
    let sha = read(&["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    let stat = read(&["show", "--shortstat", "--format=", "HEAD"]).unwrap_or_default();
    let files: Vec<String> = read(&["show", "--name-only", "--format=", "HEAD"])
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    let left = match repo.read_only(&["status", "--porcelain", "--untracked-files=all"]) {
        Ok(status) => {
            let left = left_uncommitted(&status);
            if left.is_empty() {
                "Nothing is left uncommitted.".to_owned()
            } else {
                format!("Left uncommitted: {}.", listed(&left))
            }
        }
        Err(err) => format!("What is left uncommitted is unknown: git status failed: {err}"),
    };
    format!(
        "Committed {} \"{subject}\" with your trailer: {} ({}). {left}",
        sha.trim(),
        stat.trim(),
        listed(&files)
    )
}

/// The last line with something on it in a command's output.
fn last_said(output: &[u8]) -> String {
    String::from_utf8_lossy(output)
        .lines()
        .rfind(|line| !line.trim().is_empty())
        .unwrap_or("no error output")
        .trim()
        .to_owned()
}

impl SessionManager {
    /// `run_check`'s `commit` (BUILD-PLAN C9): on a pass, everything changed in `tree` is
    /// committed with `message` and the thread's trailer, run as `caller`'s check ran; on a
    /// failure nothing is. Nor is anything when a file changed since the check started on the
    /// files of `checked` (a [`brigadier_git::Worktree::checkout_tree`]): the check didn't see
    /// that change. What happened, in one line.
    async fn commit_on_pass(
        &self,
        caller: &Caller,
        tree: &Path,
        passed: bool,
        checked: Option<String>,
        message: &str,
    ) -> String {
        if !passed {
            return "Not committed: the check didn't pass.".into();
        }
        if let Some(checked) = checked {
            let (git, root) = (self.git.clone(), tree.to_owned());
            let changed = blocking(move || {
                git.open_worktree(&root)
                    .and_then(|worktree| worktree.paths_changed_since(&brigadier_git::Oid(checked)))
                    .map_err(git_error)
            })
            .await;
            match changed {
                Ok(paths) if paths.is_empty() => {}
                Ok(paths) => {
                    return format!(
                        "Not committed: {} changed while the check ran, so it didn't check them. Run the check again with commit.",
                        listed(&paths)
                    );
                }
                Err(err) => {
                    return format!(
                        "Not committed: git could not tell whether files changed while the check ran: {err}"
                    );
                }
            }
        }
        let spec = match self.check_spec(caller, tree, &commit_command(message)) {
            Ok(spec) => spec,
            Err(err) => return format!("Not committed: {err}"),
        };
        let before = self.head_of(tree).await;
        let ran = run_command_then(
            self.runtime.platform().clone(),
            &spec,
            COMMIT_TIMEOUT,
            caller.ended.clone(),
            &caller.owner,
            self.runtime.ledger(),
            |_| {},
        )
        .await;
        let failed = match ran {
            Ok(ran) => match ran.status.as_str() {
                "exit 0" => None,
                "exit 100" => return "Nothing to commit: no change is left uncommitted.".into(),
                "exit 101" => {
                    return format!("Not committed: git add failed: {}", last_said(&ran.output));
                }
                status if status.starts_with("exit ") => Some(format!(
                    "git commit failed ({status}): {}",
                    last_said(&ran.output)
                )),
                // Timed out, killed or stopped.
                status => Some(format!("git commit {status}.")),
            },
            Err(err) => Some(err.to_string()),
        };
        // A hook that failed or was stopped after git made the commit (a post-commit hook, a
        // timeout, the session ending) leaves the commit made: HEAD says which.
        let failed = match failed {
            None => return self.commit_record_of(tree, message).await,
            Some(failed) if self.head_of(tree).await == before => {
                return format!("Not committed: {failed}");
            }
            Some(failed) => failed,
        };
        format!(
            "{} Then {failed}",
            self.commit_record_of(tree, message).await
        )
    }

    /// [`commit_record`], read off the blocking pool.
    async fn commit_record_of(&self, tree: &Path, message: &str) -> String {
        let (git, root, message) = (self.git.clone(), tree.to_owned(), message.to_owned());
        blocking(move || Ok(commit_record(&git, &root, &message)))
            .await
            .unwrap_or_else(|err| format!("Committed, but git could not read it back: {err}"))
    }

    /// The commit `tree`'s HEAD names, when git can tell.
    async fn head_of(&self, tree: &Path) -> Option<String> {
        let (git, root) = (self.git.clone(), tree.to_owned());
        blocking(move || {
            git.open(&root)
                .and_then(|repo| repo.read_only(&["rev-parse", "HEAD"]))
                .map_err(git_error)
        })
        .await
        .ok()
        .map(|head| head.trim().to_owned())
    }
}

/// The paths a `git status --porcelain --untracked-files=all` shows, tracked or untracked,
/// past its ignore rules.
fn left_uncommitted(status: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| line.get(3..))
        .map(str::to_owned)
        .collect()
}

/// Up to ten paths, then how many more.
fn listed(paths: &[String]) -> String {
    const SHOWN: usize = 10;
    let mut text = paths
        .iter()
        .take(SHOWN)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > SHOWN {
        text.push_str(&format!(" and {} more", paths.len() - SHOWN));
    }
    text
}

/// How one check ended: the reply the model gets, its status ("exit 0", "timed out after …")
/// and its whole output's alias.
pub(super) struct CheckDone {
    pub reply: String,
    pub status: String,
    pub alias: String,
}

/// What runs the session's full checks (SESSION-FIXES-PLAN.md §11): its check checkout, at the
/// session's access, in the background under the checks' own cleanup owner.
pub(super) struct FullCheckCaller(Caller);

/// What a checkout's changes are counted from.
#[derive(Debug, Clone)]
enum Base {
    /// A worker's task base.
    Commit(String),
    /// The thread's session branch's base branch: the changes since HEAD left it.
    Branch(String),
    /// Uncommitted changes only.
    Head,
}

/// A check's identity: the tree it ran on, where, and the hash of the whole key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    root: PathBuf,
    tree: String,
    /// The workdir relative to the repository's root ("" for the root).
    workdir: String,
    hash: String,
}

/// A kept result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Kept {
    status: String,
    /// What the model got.
    reply: String,
    /// The reply is sized for a model that reads it JSON-escaped (a Codex caller).
    wrapped: bool,
    conversation_id: ConversationId,
    /// The worker the reply was made for (its whole output's copy is in that worker's scratch
    /// folder); none for the thread.
    #[serde(default)]
    task_id: Option<TaskId>,
    /// What the caller's sandbox allowed ([`access_class`]): a failure is only an answer for a
    /// caller allowed the same, since the sandbox may be what failed it.
    #[serde(default)]
    access: Option<String>,
    alias: String,
    blob: String,
    duration_ms: u64,
    ran_at_ms: i64,
}

impl SessionManager {
    /// The thread's `run_check`: its risky mark first, then the check itself, and once it has
    /// ended its change's review when it is due (over the line, or marked risky) and the check
    /// passed, its reply ending with the review that runs, if one does (BUILD-PLAN B2, C2, C10).
    pub(crate) async fn thread_run_check(
        &self,
        id: &ConversationId,
        args: crate::tools::ThreadRunCheck,
    ) -> Result<String> {
        let crate::tools::ThreadCheckParts {
            check: args,
            risky,
            commit,
        } = args.split();
        if let Some(why) = &risky {
            self.mark_risky(id, why).await;
        }
        // On the user's own checkout the thread commits only when they ask, with git itself.
        let (commit, local) = match commit {
            Some(_) if self.local_checkout(id).is_some() => (None, true),
            commit => (commit, false),
        };
        self.check_and_commit(id, None, args, commit)
            .await
            .map(|reply| {
                if local {
                    format!("{reply}\n{NOT_ON_LOCAL}")
                } else {
                    reply
                }
            })
    }

    /// `run_check` for the thread of `id` (`task`: none) or the worker of `task`. The
    /// thread's goes to the background when it holds the call too long, its waits for the
    /// same check and for the build lease included ([`super::background_runs`]).
    pub(crate) async fn run_check_tool(
        &self,
        id: &ConversationId,
        task: Option<&TaskId>,
        args: RunCheck,
    ) -> Result<String> {
        self.check_and_commit(id, task, args, None).await
    }

    /// [`Self::run_check_tool`], then the thread's commit of everything it changed when the
    /// check passes (`commit`: its message), in the same call, even once it went to the
    /// background (BUILD-PLAN C9); then, for the thread, the review its change is due, which
    /// only a pass starts (C10).
    async fn check_and_commit(
        &self,
        id: &ConversationId,
        task: Option<&TaskId>,
        args: RunCheck,
        commit: Option<String>,
    ) -> Result<String> {
        let mut caller = self.check_caller(id, task).await?;
        let command = args
            .command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty());
        let Some(command) = command else {
            return self.affected_checks(&caller).await;
        };
        // Keyed and run without the prefixes that change nothing about how it runs.
        let command = canonical_command(
            command,
            &check_tmpdir(&caller),
            brigadier_sandbox::limits::open_files(),
        )
        .to_owned();
        let workdir = run_workdir(
            args.workdir.as_deref(),
            caller.tree.as_deref(),
            &caller.scratch,
        )?;
        let timeout = args
            .timeout_secs
            .map_or(RUN_TIMEOUT_DEFAULT, Duration::from_secs)
            .clamp(Duration::from_secs(1), RUN_TIMEOUT_MAX);
        if task.is_some() {
            return self
                .check_now(&caller, &workdir, &command, timeout, args.rerun)
                .await
                .map(|(_, reply)| reply);
        }
        // The thread's check is the session's, not its CLI's: a check gone to the background
        // outlives the CLI.
        let cli_ended = std::mem::replace(&mut caller.ended, CancellationToken::new());
        caller.owner = super::background_runs::background_owner(id);
        let cancel = caller.ended.clone();
        let (manager, words, conversation) = (self.arc(), command.clone(), id.clone());
        let tree = caller.tree.clone();
        let work = async move {
            let under_way = manager
                .thread_check_started(&conversation, tree.as_deref())
                .await;
            let (status, mut reply) = manager
                .check_now(&caller, &workdir, &words, timeout, args.rerun)
                .await?;
            let passed = status == "exit 0";
            if let (Some(message), Some(tree)) = (commit, tree) {
                let checked = under_way.tree().map(str::to_owned);
                let note = manager
                    .commit_on_pass(&caller, &tree, passed, checked, &message)
                    .await;
                reply = format!("{reply}\n{note}");
            }
            under_way.ended(passed);
            if let Some(review) = manager.review_before_answer(&conversation).await {
                reply = format!("{reply}\n\n{review}");
            }
            Ok((status, reply))
        };
        self.in_foreground(id, "run_check", &command, cli_ended, cancel, work)
            .await
    }

    /// Runs `command` for `caller` once no run of the same check is under way, unless it
    /// already ran on the same files (and `rerun` is off): its status and reply.
    async fn check_now(
        &self,
        caller: &Caller,
        workdir: &Path,
        command: &str,
        timeout: Duration,
        rerun: bool,
    ) -> Result<(String, String)> {
        let key = self.check_key(caller, workdir, command).await;
        // One run of a key at a time: a caller that waited answers from what the other kept.
        let _running = match &key {
            Ok(key) => tokio::select! {
                running = check_lock(&key.hash, timeout) => running,
                () = caller.ended.cancelled() => {
                    return Err(Error::Invalid(
                        "stopped: the thread's session ended while the check waited for the same check".into(),
                    ));
                }
            },
            Err(_) => None,
        };
        if let (Ok(key), false) = (&key, rerun)
            && let Some(done) = self.cached_check(caller, key, command).await
        {
            return Ok((done.status, done.reply));
        }
        let heavy = crate::machine::heavy::line_is_heavy(command);
        self.run_check_command(caller, workdir, command, timeout, key, heavy)
            .await
            .map(|done| (done.status, done.reply))
    }

    /// The caller that runs the full checks of conversation `id` in `checkout`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn full_check_caller(
        &self,
        id: &ConversationId,
        checkout: PathBuf,
        scratch: PathBuf,
        tmp: PathBuf,
        access: Access,
        owner: String,
        ended: CancellationToken,
    ) -> FullCheckCaller {
        let project = self
            .core
            .conversation(id)
            .ok()
            .and_then(|conversation| conversation.project_id);
        let secret_files = project
            .as_ref()
            .and_then(|project| self.core.project(project).ok())
            .map(|project| project.prefs.secret_files)
            .unwrap_or_default();
        let redactor = super::secrets::redactor(self.session_secret_values(id).await);
        FullCheckCaller(Caller {
            conversation_id: id.clone(),
            task_id: None,
            // Brigadier's own sandbox, below Full access: no CLI of a vendor needed.
            provider: ProviderKind::Claude,
            access,
            ended,
            owner,
            tree: Some(checkout),
            scratch,
            tmp,
            cwd_grant: None,
            redactor,
            base: Base::Head,
            project,
            secret_files,
            low_priority: true,
        })
    }

    /// The project's full checks in `caller`'s checkout, as it documents them.
    pub(super) async fn full_check_commands(
        &self,
        caller: &FullCheckCaller,
    ) -> Result<Vec<(String, String)>> {
        let caller = &caller.0;
        let tree = caller
            .tree
            .clone()
            .ok_or_else(|| Error::Invalid("the full checks have no checkout".into()))?;
        let (packages, manager) = {
            let root = tree.clone();
            blocking(move || {
                let files = listed_files(&root)?;
                Ok((read_packages(&root, &files), PackageManager::of(&root)))
            })
            .await?
        };
        let learned = self.learned_checks(caller, &packages).await;
        Ok(full_checks(&tree, &packages, manager, &learned))
    }

    /// Runs one command of the full checks, from the cache when it already ran on the same
    /// files under the same inputs (unless `rerun`). Always takes the build lease. `cache`:
    /// its result is kept (an install's is not: it changes the checkout).
    pub(super) async fn run_full_check(
        &self,
        caller: &FullCheckCaller,
        command: &str,
        workdir: &str,
        timeout: Duration,
        rerun: bool,
        cache: bool,
    ) -> Result<CheckDone> {
        let caller = &caller.0;
        let root = caller
            .tree
            .clone()
            .ok_or_else(|| Error::Invalid("the full checks have no checkout".into()))?;
        let workdir = if workdir.is_empty() {
            root
        } else {
            root.join(workdir)
        };
        let key = if cache {
            self.check_key(caller, &workdir, command).await
        } else {
            Err("an install isn't kept".to_owned())
        };
        let _running = match &key {
            Ok(key) => tokio::select! {
                running = check_lock(&key.hash, timeout) => running,
                () = caller.ended.cancelled() => {
                    return Err(Error::Invalid(
                        "stopped: the thread's session ended while the check waited for the same check".into(),
                    ));
                }
            },
            Err(_) => None,
        };
        if let (Ok(key), false) = (&key, rerun)
            && let Some(done) = self.cached_check(caller, key, command).await
        {
            return Ok(done);
        }
        self.run_check_command(caller, &workdir, command, timeout, key, true)
            .await
    }

    /// Who `run_check`'s caller is: its CLI, access and checkout.
    async fn check_caller(&self, id: &ConversationId, task: Option<&TaskId>) -> Result<Caller> {
        let conversation = self.core.conversation(id)?;
        let project = conversation.project_id.clone();
        let secret_files = project
            .as_ref()
            .and_then(|project| self.core.project(project).ok())
            .map(|project| project.prefs.secret_files)
            .unwrap_or_default();
        if let Some(task_id) = task {
            let live = self
                .existing_task_live(task_id)
                .ok_or_else(|| Error::Invalid("the task's worker isn't running".into()))?;
            let ToolAccess {
                provider,
                access,
                owner,
                ended,
            } = live
                .tool_access()
                .await
                .ok_or_else(|| Error::Invalid("the task's worker isn't running".into()))?;
            let task = self.task_by_id(id, task_id).await?;
            let redactor = self.task_redactor(&task).await;
            let workspace = task
                .workspace
                .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
            let tree = workspace.worktree.map(PathBuf::from);
            let scratch = PathBuf::from(workspace.scratch);
            let cwd_grant = cwd_grant(provider, task.kind.writes(), tree.as_deref(), &scratch);
            return Ok(Caller {
                conversation_id: id.clone(),
                task_id: Some(task_id.clone()),
                provider,
                access,
                ended,
                owner,
                tree,
                scratch,
                tmp: super::workers::test_data_dir(task_id),
                cwd_grant,
                redactor,
                base: workspace.base.map_or(Base::Head, Base::Commit),
                project,
                secret_files,
                low_priority: true,
            });
        }
        let cli = self
            .conv(id)?
            .live_cli()
            .await
            .ok_or_else(|| Error::Invalid("the session's thread has ended".into()))?;
        let launch = cli
            .launch
            .clone()
            .ok_or_else(|| Error::Invalid("only a session's thread runs checks".into()))?;
        let base = match &conversation.setup {
            Some(Setup::Session {
                environment: Environment::NewWorktree { base, .. },
                ..
            }) => Base::Branch(base.clone()),
            _ => Base::Head,
        };
        let redactor = super::secrets::redactor(self.session_secret_values(id).await);
        Ok(Caller {
            conversation_id: id.clone(),
            task_id: None,
            provider: cli.provider,
            access: launch.access,
            ended: cli.ended.clone(),
            owner: cli.owner.clone(),
            tree: launch.workspace.map(|workspace| workspace.path),
            scratch: self.owned_dir("orch", &id.0),
            tmp: self.thread_temp(id).await?,
            cwd_grant: None,
            redactor,
            base,
            project,
            secret_files,
            low_priority: false,
        })
    }

    /// The key of `command` in `workdir` for `caller`, or why there is none.
    async fn check_key(
        &self,
        caller: &Caller,
        workdir: &Path,
        command: &str,
    ) -> std::result::Result<Key, String> {
        let root = repo_root(workdir).ok_or("not in a git repository")?;
        let git = self.git.clone();
        let (files, tree) = {
            let (root, workdir, secrets) = (
                root.clone(),
                workdir.to_owned(),
                caller.secret_files.clone(),
            );
            blocking(move || {
                let tree = git
                    .open_worktree(&root)
                    .and_then(|worktree| worktree.checkout_tree())
                    .map_err(git_error)?;
                let packages = package_dirs(&root)?;
                Ok((input_files(&root, &workdir, &packages, &secrets), tree.0))
            })
            .await
            .map_err(|err| format!("git could not read the tree: {err}"))?
        };
        let relative = workdir
            .strip_prefix(&root)
            .map_err(|_| "the workdir is outside its repository")?
            .to_string_lossy()
            .into_owned();
        let mut parts = vec![
            KEY_RECIPE.to_owned(),
            tree.clone(),
            command.to_owned(),
            relative.clone(),
        ];
        parts.extend(files);
        parts.extend(user_config_files());
        for (name, version, words) in TOOLCHAINS {
            match self.toolchain_version(&root, version).await {
                Some(version) => parts.push(format!("{name} {version}")),
                None if uses(command, words) => {
                    return Err(format!("could not tell the {name} version"));
                }
                None => parts.push(format!("{name} none")),
            }
        }
        Ok(Key {
            root,
            tree,
            workdir: relative,
            hash: blake3::hash(parts.join("\0").as_bytes())
                .to_hex()
                .to_string(),
        })
    }

    /// The version `command` prints in `root`, asked at most every [`TOOLCHAIN_TTL`]; `None`
    /// when it fails (the toolchain isn't there).
    async fn toolchain_version(&self, root: &Path, command: &str) -> Option<String> {
        type Versions = Mutex<HashMap<(PathBuf, String), (Instant, Option<String>)>>;
        static VERSIONS: OnceLock<Versions> = OnceLock::new();
        let versions = VERSIONS.get_or_init(Versions::default);
        let slot = (root.to_owned(), command.to_owned());
        if let Some((at, version)) = versions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&slot)
            && at.elapsed() < TOOLCHAIN_TTL
        {
            return version.clone();
        }
        let mut spec = self.runtime.cli_env().spec(Path::new("/bin/sh"));
        spec.args = vec!["-c".into(), command.into()];
        spec.cwd = Some(root.to_owned());
        let mut process =
            tokio::process::Command::from(self.runtime.platform().processes().piped_command(&spec));
        process
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true);
        let version = match tokio::time::timeout(VERSION_TIMEOUT, process.output()).await {
            Ok(Ok(output)) if output.status.success() => {
                Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
                    .filter(|version| !version.is_empty())
            }
            _ => None,
        };
        versions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(slot, (Instant::now(), version.clone()));
        version
    }

    /// The kept result of `key`, as the reply to `caller`; `None` when there is none, or its
    /// output is gone.
    async fn cached_check(&self, caller: &Caller, key: &Key, command: &str) -> Option<CheckDone> {
        let (body, _) = match self.core.store().check_result(key.hash.clone()).await {
            Ok(found) => found?,
            Err(err) => {
                tracing::warn!(error = %err, "could not read the check cache");
                return None;
            }
        };
        let kept: Kept = serde_json::from_str(&body).ok()?;
        let same = reuse(&kept, caller)?;
        let blob = kept.blob.parse().ok()?;
        // Its output went with its conversation: the check runs again.
        if !self.core.store().blobs().touch(blob).await.unwrap_or(false) {
            return None;
        }
        let (reply, alias) = if same {
            (kept.reply.clone(), kept.alias.clone())
        } else {
            // Another conversation's or caller's output: this one gets its own alias of it,
            // and a worker its own copy of the whole of it.
            let blob = kept.blob.parse().ok()?;
            let output = self.core.store().blobs().get(blob).await.ok()??;
            self.check_reply(caller, &kept.status, output).await.ok()?
        };
        self.record_check(
            caller,
            command,
            Some(key),
            Some(&alias),
            &kept.status,
            kept.duration_ms,
            true,
            None,
        )
        .await;
        Some(CheckDone {
            reply: format!(
                "[cached: this command already ran on these same files {} ago; its result is below. \
                 Pass rerun: true to run it again.]\n{reply}",
                ago(now_ms() - kept.ran_at_ms)
            ),
            status: kept.status,
            alias,
        })
    }

    /// Runs `command` in `workdir` for `caller`, keeps its result under `key` (when there is
    /// one and the command ran to its end) and returns how it ended.
    async fn run_check_command(
        &self,
        caller: &Caller,
        workdir: &Path,
        command: &str,
        timeout: Duration,
        key: std::result::Result<Key, String>,
        heavy: bool,
    ) -> Result<CheckDone> {
        let spec = self.check_spec(caller, workdir, command)?;
        // A heavy check waits for the build lease before it starts, and is never stopped or
        // paused once running (a test stopped mid-run fails its timing assertions).
        let lease = if heavy {
            let (ticket, now, rows) = self.machine.ask_check(&caller.owner, command);
            let lease = CheckLease {
                machine: self.machine.clone(),
                ticket,
            };
            if !now {
                for row in rows {
                    self.machine_row(row).await;
                }
                tokio::select! {
                    () = self.machine.wait_check(ticket) => {}
                    () = caller.ended.cancelled() => {
                        return Err(Error::Invalid(
                            "stopped: the thread's session ended while the check waited for another build".into(),
                        ));
                    }
                }
            }
            Some(lease)
        } else {
            None
        };
        let started = Instant::now();
        let ran = run_command_then(
            self.runtime.platform().clone(),
            &spec,
            timeout,
            caller.ended.clone(),
            &caller.owner,
            self.runtime.ledger(),
            |pid| {
                if let Some(lease) = &lease {
                    self.machine.attach_check(lease.ticket, pid);
                }
            },
        )
        .await;
        drop(lease);
        let ran = ran?;
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (reply, alias) = self.check_reply(caller, &ran.status, ran.output).await?;
        let (key, bypassed) = match key {
            Ok(key) => (Some(key), None),
            Err(reason) => (None, Some(reason)),
        };
        if let Some(key) = &key
            && ran.status.starts_with("exit ")
        {
            self.keep_check(caller, key, &ran.status, &reply, &alias, duration_ms)
                .await;
        }
        self.record_check(
            caller,
            command,
            key.as_ref(),
            Some(&alias),
            &ran.status,
            duration_ms,
            false,
            bypassed.as_deref(),
        )
        .await;
        // The full checks' commands are the project's, not a package's to learn.
        let full_checks = caller.task_id.is_none() && caller.low_priority;
        if ran.status == "exit 0" && !full_checks {
            self.learn_check(caller, workdir, command).await;
        }
        Ok(CheckDone {
            reply,
            status: ran.status,
            alias,
        })
    }

    /// What runs `command` in `workdir` for `caller`: in its checkout's root (the sandbox's
    /// working folder, so a build writes the workspace's own target folder), or in the folder
    /// its access grants as its working directory when that isn't its checkout, changing into
    /// `workdir` first, at the caller's access.
    fn check_spec(
        &self,
        caller: &Caller,
        workdir: &Path,
        command: &str,
    ) -> Result<brigadier_sandbox::SpawnSpec> {
        let root = check_root(caller.tree.as_deref(), caller.cwd_grant.as_deref(), workdir);
        let command = if root == workdir {
            command.to_owned()
        } else {
            format!("cd {} || exit 125\n{command}", quote(workdir))
        };
        let mut spec = match (&caller.access, caller.provider) {
            (Access::Full, _) | (_, ProviderKind::Codex) => {
                self.run_spec(&caller.access, &root, &command)?
            }
            (access, ProviderKind::Claude) => self
                .seatbelt_spec(access, &root, &shell_command(&command)[1..])
                .map_err(|err| {
                    Error::Invalid(format!("the check can't be sandboxed here: {err}"))
                })?,
        };
        let mut env = super::workers::worker_env(&caller.scratch);
        for (name, value) in &mut env {
            if name == "TMPDIR" {
                *value = check_tmpdir(caller).to_string_lossy().into_owned();
            }
        }
        brigadier_providers::cli::apply_session_env(&mut spec, &env, &[]);
        // Workers run at low priority, and so do their checks and the full checks.
        spec.low_priority = caller.low_priority;
        Ok(spec)
    }

    /// Stores a check's whole output for `caller`'s conversation; returns the reply (the
    /// output itself up to [`TRIM_ABOVE`] bytes, else its digest; a pass's over
    /// [`crate::digest::PASS_WHOLE_MAX`] bytes as its warnings and summary) and the output's
    /// alias.
    async fn check_reply(
        &self,
        caller: &Caller,
        status: &str,
        output: Vec<u8>,
    ) -> Result<(String, String)> {
        // Nothing of the project's secrets or the session's grants is shown, stored or copied.
        let output = redact(
            output,
            &[
                caller.redactor.clone(),
                super::secrets::redactor(self.grants.secrets()),
            ],
        );
        // A pass's long output comes as its warnings and summary (BUILD-PLAN C1).
        let passed = status == "exit 0" && output.len() > crate::digest::PASS_WHOLE_MAX;
        let trimmed = passed || output.len() > TRIM_ABOVE;
        let short = (!trimmed).then(|| format!("[{status}]\n{}", String::from_utf8_lossy(&output)));
        let whole = (trimmed && caller.task_id.is_some()).then(|| output.clone());
        let (stored, digest) = self
            .store_output_as(
                &caller.conversation_id,
                OutputSource::Check,
                status,
                output,
                trimmed,
                caller.provider == ProviderKind::Codex,
                passed,
            )
            .await?;
        let mut reply = short.unwrap_or(digest);
        // A worker has no read_artifact: it reads the whole output from its scratch folder.
        if let Some(whole) = whole {
            let path = caller
                .scratch
                .join("checks")
                .join(format!("{}.log", stored.alias));
            let written = {
                let path = path.clone();
                blocking(move || {
                    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))
                        .and_then(|()| std::fs::write(&path, whole))
                        .map_err(|err| Error::Invalid(err.to_string()))
                })
                .await
            };
            if written.is_ok() {
                reply.push_str(&format!("\n[the whole output: {}]", path.display()));
            }
        }
        Ok((reply, stored.alias))
    }

    /// Keeps a finished check's result under its key. Best effort: a check never fails for it.
    async fn keep_check(
        &self,
        caller: &Caller,
        key: &Key,
        status: &str,
        reply: &str,
        alias: &str,
        duration_ms: u64,
    ) {
        if !keepable(status) {
            return;
        }
        let blob = match self.core.board(&caller.conversation_id).await {
            Ok(board) => board.outputs.get(alias).map(|output| output.blob.clone()),
            Err(_) => None,
        };
        let Some(blob) = blob else {
            return;
        };
        let kept = Kept {
            status: status.to_owned(),
            reply: reply.to_owned(),
            wrapped: caller.provider == ProviderKind::Codex,
            conversation_id: caller.conversation_id.clone(),
            task_id: caller.task_id.clone(),
            access: Some(access_class(caller)),
            alias: alias.to_owned(),
            blob,
            duration_ms,
            ran_at_ms: now_ms(),
        };
        let Ok(body) = serde_json::to_string(&kept) else {
            return;
        };
        if let Err(err) = self
            .core
            .store()
            .put_check_result(key.hash.clone(), body)
            .await
        {
            tracing::warn!(error = %err, "could not keep a check's result");
        }
    }

    /// Records a run or a cached answer on the conversation, and logs it.
    #[allow(clippy::too_many_arguments)]
    async fn record_check(
        &self,
        caller: &Caller,
        command: &str,
        key: Option<&Key>,
        artifact: Option<&str>,
        status: &str,
        duration_ms: u64,
        cached: bool,
        bypassed: Option<&str>,
    ) {
        tracing::info!(
            conversation = %caller.conversation_id,
            task = caller.task_id.as_ref().map(|task| task.0.as_str()),
            tree = key.map(|key| key.tree.as_str()),
            workdir = key.map(|key| key.workdir.as_str()),
            command,
            cached,
            bypassed,
            status,
            duration_ms,
            "check ran"
        );
        let event = DomainEvent::CheckRan {
            conversation_id: caller.conversation_id.clone(),
            task_id: caller.task_id.clone(),
            tree: key.map(|key| key.tree.clone()),
            command: command.to_owned(),
            workdir: key.map(|key| key.workdir.clone()).unwrap_or_default(),
            cached,
            bypassed: bypassed.map(str::to_owned),
            status: status.to_owned(),
            duration_ms,
            artifact: artifact.map(str::to_owned),
        };
        if let Err(err) = self
            .core
            .record_conversation(&caller.conversation_id, vec![event])
            .await
        {
            tracing::warn!(conversation = %caller.conversation_id, error = %err, "could not record a check");
        }
    }

    /// Learns that `command` passed in `workdir`'s package folder, into the project's Brain.
    /// Best effort, in the background: a Brain error never fails a check.
    async fn learn_check(&self, caller: &Caller, workdir: &Path, command: &str) {
        let (Some(project), Some(root)) = (caller.project.clone(), repo_root(workdir)) else {
            return;
        };
        let folder = package_folder(&root, workdir);
        let provenance = Provenance {
            origin: if caller.task_id.is_some() {
                Origin::Report
            } else {
                Origin::Orchestrator
            },
            session_id: Some(caller.conversation_id.0.clone()),
            task_id: caller.task_id.as_ref().map(|task| task.0.clone()),
            job_id: None,
            worker: None,
            commit: None,
            recorded_at_ms: now_ms(),
            claim: Some(Claim::verified()),
        };
        let this = self.arc();
        let command = command.to_owned();
        self.spawn(async move {
            let brain = match this.project_brain(&project).await {
                Ok(brain) => brain.brain.clone(),
                Err(err) => {
                    tracing::debug!(error = %err, "no Brain to learn a check into");
                    return;
                }
            };
            let learned = blocking(move || {
                let key = learned_key(&folder);
                let known = brain
                    .node_by_key(NodeKind::Convention, &key)
                    .map_err(super::brains::brain_error)?;
                let mut commands = known
                    .as_ref()
                    .map(|node| learned_commands(&node.body))
                    .unwrap_or_default();
                if commands.contains(&command) {
                    return Ok(());
                }
                commands.push(command);
                brain
                    .record(NewNode {
                        kind: NodeKind::Convention,
                        key: Some(key),
                        title: format!("Checks that pass in {folder}"),
                        body: learned_body(&commands),
                        provenance,
                        files: Vec::new(),
                        expires_at_ms: None,
                    })
                    .map(|_| ())
                    .map_err(super::brains::brain_error)
            })
            .await;
            if let Err(err) = learned {
                tracing::warn!(error = %err, "could not learn a check");
            }
        });
    }

    /// `run_check` with no command: the checks the changes since `caller`'s base call for.
    async fn affected_checks(&self, caller: &Caller) -> Result<String> {
        let tree = caller.tree.clone().ok_or_else(|| {
            Error::Invalid("there is no checkout here to look for changes in".into())
        })?;
        let root = repo_root(&tree).ok_or_else(|| {
            Error::Invalid(format!("{} is not in a git repository", tree.display()))
        })?;
        let git = self.git.clone();
        let base = caller.base.clone();
        let (changed, packages, manager) = {
            let root = root.clone();
            blocking(move || {
                let changed = changed_paths(&git, &root, &base)?;
                let files = listed_files(&root)?;
                let packages = read_packages(&root, &files);
                Ok((changed, packages, PackageManager::of(&root)))
            })
            .await?
        };
        let learned = self.learned_checks(caller, &packages).await;
        let plan = affected(&changed, &packages, manager, &learned);
        Ok(plan.text(changed.len()))
    }

    /// What the project's Brain learned of checks per package folder.
    async fn learned_checks(
        &self,
        caller: &Caller,
        packages: &[Package],
    ) -> BTreeMap<String, Vec<String>> {
        let Some(project) = &caller.project else {
            return BTreeMap::new();
        };
        let Ok(brain) = self.project_brain(project).await else {
            return BTreeMap::new();
        };
        let brain = brain.brain.clone();
        let mut folders: BTreeSet<String> = packages.iter().map(|p| p.folder()).collect();
        folders.insert(".".into());
        blocking(move || {
            let mut learned = BTreeMap::new();
            for folder in folders {
                if let Ok(Some(node)) =
                    brain.node_by_key(NodeKind::Convention, &learned_key(&folder))
                {
                    learned.insert(folder, learned_commands(&node.body));
                }
            }
            Ok(learned)
        })
        .await
        .unwrap_or_default()
    }
}

/// Where a check of a caller with checkout `tree` runs, for `workdir`: the folder its access
/// grants as its working directory when that isn't its checkout (`cwd_grant`), else the
/// checkout's root when `workdir` is in it, else `workdir` itself.
/// The temporary folder a check runs with ([`Caller::tmp`]).
fn check_tmpdir(caller: &Caller) -> PathBuf {
    caller.tmp.clone()
}

/// `command` without the leading prefixes proven to change nothing about how it runs (bug 8):
/// a `TMPDIR` set to exactly `tmpdir`, which the check sets anyway, and a `ulimit -n` that
/// leaves both the soft and the hard limit as they are (`limits`, which the check inherits).
/// `ulimit -n N` without `-S` or `-H` sets both limits in bash, the soft one in zsh, so it is
/// dropped only when N is both. Anything else stays, in the command and so in its cache key:
/// another folder, a limit that lowers or raises either one, other variables.
fn canonical_command<'a>(
    command: &'a str,
    tmpdir: &Path,
    limits: Option<brigadier_sandbox::limits::OpenFiles>,
) -> &'a str {
    let mut rest = command.trim_start();
    loop {
        let next = limits
            .and_then(|limits| drop_ulimit(rest, limits))
            .or_else(|| drop_tmpdir(rest, tmpdir));
        match next {
            Some(after) => rest = after.trim_start(),
            None => return rest,
        }
    }
}

/// The next word of a command line and what follows it; a word ends at a blank, `;` or `&`.
fn next_word(line: &str) -> (&str, &str) {
    let line = line.trim_start_matches([' ', '\t']);
    let end = line.find([' ', '\t', '\n', ';', '&']).unwrap_or(line.len());
    line.split_at(end)
}

/// What follows a command separator (`;`, `&&` or a new line) at the start of `line`.
fn after_separator(line: &str) -> Option<&str> {
    let line = line.trim_start_matches([' ', '\t']);
    line.strip_prefix("&&")
        .or_else(|| line.strip_prefix(';'))
        .or_else(|| line.strip_prefix('\n'))
}

/// A word without its surrounding quotes.
fn unquoted(word: &str) -> &str {
    for quote in ['\'', '"'] {
        if let Some(inner) = word
            .strip_prefix(quote)
            .and_then(|word| word.strip_suffix(quote))
        {
            return inner;
        }
    }
    word
}

/// What follows a leading `ulimit -n N;` that leaves both limits as `limits` has them.
fn drop_ulimit(line: &str, limits: brigadier_sandbox::limits::OpenFiles) -> Option<&str> {
    let (word, mut rest) = next_word(line);
    if word != "ulimit" {
        return None;
    }
    let (mut soft, mut hard, mut files) = (false, false, false);
    let value = loop {
        let (word, after) = next_word(rest);
        rest = after;
        match word.strip_prefix('-') {
            Some(flags) if !flags.is_empty() => {
                for flag in flags.chars() {
                    match flag {
                        'S' => soft = true,
                        'H' => hard = true,
                        'n' => files = true,
                        _ => return None,
                    }
                }
            }
            Some(_) => return None,
            None => break word,
        }
    };
    if !files {
        return None;
    }
    let value = match value {
        "unlimited" => brigadier_sandbox::limits::UNLIMITED,
        number => number.parse().ok()?,
    };
    let (soft, hard) = if soft || hard {
        (soft, hard)
    } else {
        (true, true)
    };
    let unchanged = (!soft || value == limits.soft) && (!hard || value == limits.hard);
    if unchanged {
        after_separator(rest)
    } else {
        None
    }
}

/// What follows a leading `TMPDIR=<tmpdir> ` (before the command it applies to),
/// `TMPDIR=<tmpdir>;` or `export TMPDIR=<tmpdir>;`, for exactly the folder the check sets.
fn drop_tmpdir<'a>(line: &'a str, tmpdir: &Path) -> Option<&'a str> {
    let (mut word, mut rest) = next_word(line);
    let exported = word == "export";
    if exported {
        (word, rest) = next_word(rest);
    }
    let value = word.strip_prefix("TMPDIR=")?;
    if Path::new(unquoted(value)) != tmpdir {
        return None;
    }
    match after_separator(rest) {
        Some(after) => Some(after),
        // `TMPDIR=… command`: the assignment applies to the command that follows.
        None if !exported && rest.starts_with([' ', '\t']) && !rest.trim().is_empty() => Some(rest),
        None => None,
    }
}

fn check_root(tree: Option<&Path>, cwd_grant: Option<&Path>, workdir: &Path) -> PathBuf {
    if let Some(grant) = cwd_grant {
        return grant.canonicalize().unwrap_or_else(|_| grant.to_owned());
    }
    tree.and_then(|tree| tree.canonicalize().ok())
        .filter(|tree| workdir.starts_with(tree))
        .unwrap_or_else(|| workdir.to_owned())
}

/// The folder a worker's access grants as its writable working directory when that isn't its
/// checkout `tree` (see `launch_admitted`): a Codex worker that doesn't write works from its
/// `scratch` folder, since Codex can't run in a read-only folder. A Claude worker's access is
/// made for its checkout, and a writing Codex worker starts in it.
fn cwd_grant(
    provider: ProviderKind,
    writes: bool,
    tree: Option<&Path>,
    scratch: &Path,
) -> Option<PathBuf> {
    match (tree, provider, writes) {
        (Some(_), ProviderKind::Claude, _) | (Some(_), _, true) => None,
        _ => Some(scratch.to_owned()),
    }
}

/// Whether a check that ended with `status` is kept for the same files. A command the shell
/// couldn't find or run (126, 127: a tool not installed in that checkout, such as dependencies
/// never installed in the thread's workspace) says nothing of the code, so it runs again: the
/// full checks' checkout, whose dependencies are installed, once took such a failure for its
/// own.
fn keepable(status: &str) -> bool {
    !matches!(status, "exit 126" | "exit 127")
}

/// Whether `kept` answers `caller`: `None` when the check runs again (a failure under another
/// sandbox, which may be what failed it); else whether its reply is the caller's own as it is
/// (`true`), or is made again from its output (another conversation's, or another worker's
/// whose copy of the whole output is in that worker's scratch folder, or the thread's that has
/// none: a worker reads the whole output only from its own).
fn reuse(kept: &Kept, caller: &Caller) -> Option<bool> {
    reuse_for(
        kept,
        &caller.conversation_id,
        caller.task_id.as_ref(),
        caller.provider == ProviderKind::Codex,
        &access_class(caller),
    )
}

fn reuse_for(
    kept: &Kept,
    conversation: &ConversationId,
    task: Option<&TaskId>,
    wrapped: bool,
    class: &str,
) -> Option<bool> {
    if kept.status != "exit 0" && kept.access.as_deref() != Some(class) {
        return None;
    }
    Some(
        kept.conversation_id == *conversation
            && kept.wrapped == wrapped
            && kept.task_id.as_ref() == task,
    )
}

/// What `caller`'s sandbox allows a check: everything, or the network or not and writing its
/// checkout or not.
fn access_class(caller: &Caller) -> String {
    class_of(
        &caller.access,
        caller.tree.as_deref(),
        caller.cwd_grant.is_some(),
    )
}

/// [`access_class`] of `access`, for checkout `tree`, its working-directory grant elsewhere
/// when `grant_elsewhere`.
fn class_of(access: &Access, tree: Option<&Path>, grant_elsewhere: bool) -> String {
    let writes = |roots: &[PathBuf]| {
        tree.is_some_and(|tree| roots.iter().any(|root| tree.starts_with(root)))
    };
    match access {
        Access::Full => "full".into(),
        Access::Workspace { .. } => "sandboxed network writes-checkout".into(),
        Access::Scoped {
            write_cwd,
            writable_roots,
            network,
            ..
        } => format!(
            "sandboxed{}{}",
            if *network { " network" } else { "" },
            if (*write_cwd && !grant_elsewhere) || writes(writable_roots) {
                " writes-checkout"
            } else {
                ""
            }
        ),
        Access::ReadOnly => "read-only".into(),
    }
}

/// Waits, up to `timeout`, until no other check of the key `hash` runs, and holds that until
/// dropped. `None` when the wait timed out (the check runs anyway).
async fn check_lock(hash: &str, timeout: Duration) -> Option<tokio::sync::OwnedMutexGuard<()>> {
    type Running = Mutex<HashMap<String, std::sync::Weak<tokio::sync::Mutex<()>>>>;
    static RUNNING: OnceLock<Running> = OnceLock::new();
    let lock = {
        let mut running = RUNNING
            .get_or_init(Running::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        running.retain(|_, lock| lock.strong_count() > 0);
        match running.get(hash).and_then(std::sync::Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                running.insert(hash.to_owned(), Arc::downgrade(&lock));
                lock
            }
        }
    };
    tokio::time::timeout(timeout, lock.lock_owned()).await.ok()
}

/// `output` with every value the `redactors` know replaced, when it is text.
fn redact(output: Vec<u8>, redactors: &[Option<Arc<Redactor>>]) -> Vec<u8> {
    // Output that isn't text is kept as it is, as `store_output_as` keeps it.
    let mut text = match String::from_utf8(output) {
        Ok(text) => text,
        Err(err) => return err.into_bytes(),
    };
    for redactor in redactors.iter().flatten() {
        text = redactor.redact(&text).into_owned();
    }
    text.into_bytes()
}

/// The top of the git checkout `dir` is in: the nearest folder up that holds `.git`.
fn repo_root(dir: &Path) -> Option<PathBuf> {
    let dir = dir.canonicalize().ok()?;
    dir.ancestors()
        .find(|folder| folder.join(".git").exists())
        .map(Path::to_owned)
}

/// What the key holds besides the tree: each secret file of the project, ignored `.env*`
/// file and lockfile in the root, in `workdir` and in each package folder (`packages`,
/// repository-relative: a check run from the root may build any of them), by its content hash
/// ("absent" for a listed secret file that isn't there). Sorted, one line each.
fn input_files(
    root: &Path,
    workdir: &Path,
    packages: &[String],
    secrets: &[String],
) -> Vec<String> {
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    let mut add = |path: PathBuf| {
        let Ok(relative) = path.strip_prefix(root) else {
            return;
        };
        let hash = std::fs::read(&path).map_or_else(
            |_| "absent".to_owned(),
            |bytes| blake3::hash(&bytes).to_hex().to_string(),
        );
        files.insert(relative.to_string_lossy().into_owned(), hash);
    };
    for secret in secrets {
        let secret = Path::new(secret);
        if secret.is_relative()
            && !secret
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            add(root.join(secret));
        }
    }
    let packages: Vec<PathBuf> = packages.iter().map(|dir| root.join(dir)).collect();
    for folder in [root, workdir]
        .into_iter()
        .chain(packages.iter().map(PathBuf::as_path))
    {
        for name in LOCKFILES.iter().chain(CONFIG_FILES) {
            let path = folder.join(name);
            if path.is_file() {
                add(path);
            }
        }
        let Ok(entries) = std::fs::read_dir(folder) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with(".env")
                && entry.file_type().is_ok_and(|kind| kind.is_file())
            {
                add(entry.path());
            }
        }
    }
    files
        .into_iter()
        .map(|(path, hash)| format!("file {path} {hash}"))
        .collect()
}

/// The user's own package manager and Cargo configuration, by content hash: one line each
/// that exists.
fn user_config_files() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return vec!["home unknown".into()];
    };
    CONFIG_FILES
        .iter()
        .filter_map(|name| {
            let bytes = std::fs::read(home.join(name)).ok()?;
            Some(format!("home {name} {}", blake3::hash(&bytes).to_hex()))
        })
        .collect()
}

/// Whether `command` names one of `words` as a command word.
fn uses(command: &str, words: &[&str]) -> bool {
    command
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .any(|word| words.contains(&word))
}

/// `path` quoted for `/bin/sh`.
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// "12 s", "4 min", "3 h", "2 d".
fn ago(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    match secs {
        0..60 => format!("{secs} s"),
        60..3_600 => format!("{} min", secs / 60),
        3_600..86_400 => format!("{} h", secs / 3_600),
        _ => format!("{} d", secs / 86_400),
    }
}

/// The Brain key of the checks learned for a package folder.
fn learned_key(folder: &str) -> String {
    format!("checks:{folder}")
}

/// The commands a learned-checks node lists, one per line.
fn learned_commands(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("- "))
        .map(|command| command.trim_matches('`').to_owned())
        .filter(|command| !command.is_empty())
        .collect()
}

fn learned_body(commands: &[String]) -> String {
    let mut body = String::from("Checks that passed here through run_check:");
    for command in commands {
        body.push_str(&format!("\n- `{command}`"));
    }
    body
}

/// The nearest folder from `workdir` up to `root` that holds a package manifest, relative to
/// `root` ("." for the root).
fn package_folder(root: &Path, workdir: &Path) -> String {
    let workdir = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_owned());
    let found = workdir
        .ancestors()
        .take_while(|folder| folder.starts_with(root))
        .find(|folder| folder.join("package.json").is_file() || folder.join("Cargo.toml").is_file())
        .unwrap_or(root);
    folder_name(found.strip_prefix(root).unwrap_or(Path::new("")))
}

/// A repository-relative folder as the learned map names it.
fn folder_name(relative: &Path) -> String {
    let name = relative.to_string_lossy().replace('\\', "/");
    if name.is_empty() { ".".into() } else { name }
}

/// The paths changed since `base` in the checkout at `root`, uncommitted and untracked
/// changes included.
fn changed_paths(git: &brigadier_git::Git, root: &Path, base: &Base) -> Result<Vec<String>> {
    let worktree = git.open_worktree(root).map_err(git_error)?;
    let head = worktree.head().map_err(git_error)?;
    let from = match base {
        Base::Commit(commit) => brigadier_git::Oid(commit.clone()),
        Base::Head => head,
        Base::Branch(branch) => {
            let repo = git.open(root).map_err(git_error)?;
            match repo.branch_tip(branch).map_err(git_error)? {
                Some(tip) => repo.merge_base(&head, &tip).map_err(git_error)?,
                None => head,
            }
        }
    };
    let mut paths = BTreeSet::new();
    for change in worktree.changes(&from).map_err(git_error)? {
        if let brigadier_git::ChangeKind::Renamed { from } = &change.kind {
            paths.insert(from.clone());
        }
        paths.insert(change.path);
    }
    Ok(paths.into_iter().collect())
}

/// Every file of the checkout git doesn't ignore, repository-relative.
fn listed_files(root: &Path) -> Result<Vec<String>> {
    git_files(root, &[])
}

/// The folders of the checkout's package manifests, repository-relative ("" for the root).
fn package_dirs(root: &Path) -> Result<Vec<String>> {
    let manifests = git_files(root, &[":(glob)**/package.json", ":(glob)**/Cargo.toml"])?;
    let dirs: BTreeSet<String> = manifests
        .iter()
        .map(|file| {
            Path::new(file)
                .parent()
                .map(|dir| dir.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default()
        })
        .collect();
    Ok(dirs.into_iter().collect())
}

/// The files of the checkout git doesn't ignore that match `pathspecs` (all without),
/// repository-relative.
fn git_files(root: &Path, pathspecs: &[&str]) -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
        ])
        .args(pathspecs)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .map_err(|err| Error::Invalid(format!("could not run git: {err}")))?;
    if !output.status.success() {
        return Err(Error::Invalid(format!(
            "git could not list the files: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect())
}

/// A package of the workspace: a Cargo crate or a Node package, by its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Package {
    /// Repository-relative, `/`-separated; "" for the root.
    dir: String,
    name: Option<String>,
    kind: PackageKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PackageKind {
    /// A crate, and the folders of the crates it depends on by path.
    Cargo { path_deps: Vec<String> },
    /// A Node package, its check scripts, and the names of the workspace packages it uses.
    Node {
        scripts: Vec<String>,
        workspace_deps: Vec<String>,
    },
}

impl Package {
    fn folder(&self) -> String {
        folder_name(Path::new(&self.dir))
    }

    /// Whether `other` depends on this package.
    fn used_by(&self, other: &Package) -> bool {
        match (&self.kind, &other.kind) {
            (PackageKind::Cargo { .. }, PackageKind::Cargo { path_deps }) => {
                path_deps.contains(&self.dir)
            }
            (PackageKind::Node { .. }, PackageKind::Node { workspace_deps, .. }) => self
                .name
                .as_ref()
                .is_some_and(|name| workspace_deps.contains(name)),
            _ => false,
        }
    }
}

/// The package manager a Node workspace uses, by its lockfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageManager {
    Pnpm,
    Yarn,
    Npm,
}

impl PackageManager {
    fn of(root: &Path) -> Self {
        if root.join("pnpm-lock.yaml").exists() || root.join("pnpm-workspace.yaml").exists() {
            Self::Pnpm
        } else if root.join("yarn.lock").exists() {
            Self::Yarn
        } else {
            Self::Npm
        }
    }

    fn run(self, script: &str) -> String {
        match self {
            Self::Pnpm => format!("pnpm run {script}"),
            Self::Yarn => format!("yarn run {script}"),
            Self::Npm => format!("npm run {script}"),
        }
    }
}

/// The packages among `files`: each `Cargo.toml` with a `[package]` and each `package.json`.
/// Only those manifests are read.
fn read_packages(root: &Path, files: &[String]) -> Vec<Package> {
    let manifests: Vec<(String, &str, String)> = files
        .iter()
        .filter_map(|file| {
            let path = Path::new(file);
            let name = match path.file_name().and_then(|name| name.to_str()) {
                Some("Cargo.toml") => "Cargo.toml",
                Some("package.json") => "package.json",
                _ => return None,
            };
            let dir = path
                .parent()
                .map(|dir| dir.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let text = std::fs::read_to_string(root.join(path)).ok()?;
            Some((dir, name, text))
        })
        .collect();
    // The crates' `workspace = true` dependencies are the workspaces' own.
    let workspaces: BTreeMap<String, BTreeMap<String, String>> = manifests
        .iter()
        .filter(|(_, name, _)| *name == "Cargo.toml")
        .filter_map(|(dir, _, text)| Some((dir.clone(), workspace_path_deps(dir, text)?)))
        .collect();
    manifests
        .iter()
        .filter_map(|(dir, name, text)| match *name {
            "Cargo.toml" => {
                // The nearest workspace above the crate (a folder holds its own).
                let workspace = workspaces
                    .iter()
                    .filter(|(root, _)| {
                        root.is_empty() || *dir == **root || dir.starts_with(&format!("{root}/"))
                    })
                    .max_by_key(|(root, _)| root.len())
                    .map(|(_, deps)| deps);
                cargo_package_in(dir, text, workspace)
            }
            _ => node_package(dir, text),
        })
        .collect()
}

/// The path dependencies a workspace's `Cargo.toml` in `dir` declares for its crates
/// (`[workspace.dependencies]`), by name, their folders repository-relative; `None` when it
/// isn't a workspace's manifest.
fn workspace_path_deps(dir: &str, text: &str) -> Option<BTreeMap<String, String>> {
    let manifest: toml::Table = toml::from_str(text).ok()?;
    let workspace = manifest.get("workspace")?.as_table()?;
    Some(
        workspace
            .get("dependencies")
            .and_then(|deps| deps.as_table())
            .into_iter()
            .flatten()
            .filter_map(|(name, dependency)| {
                let path = dependency.get("path")?.as_str()?;
                Some((name.clone(), normalize(&format!("{dir}/{path}"))))
            })
            .collect(),
    )
}

/// A crate from its `Cargo.toml` in `dir`; `None` for a workspace's own manifest.
#[cfg(test)]
fn cargo_package(dir: &str, text: &str) -> Option<Package> {
    cargo_package_in(dir, text, None)
}

/// A crate from its `Cargo.toml` in `dir`, its `workspace = true` dependencies looked up in
/// its `workspace`'s path dependencies; `None` for a workspace's own manifest.
fn cargo_package_in(
    dir: &str,
    text: &str,
    workspace: Option<&BTreeMap<String, String>>,
) -> Option<Package> {
    let manifest: toml::Table = toml::from_str(text).ok()?;
    let name = manifest
        .get("package")?
        .get("name")
        .and_then(|name| name.as_str())
        .map(str::to_owned);
    let mut path_deps = Vec::new();
    let tables = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut sections: Vec<&toml::Table> = tables
        .iter()
        .filter_map(|table| manifest.get(*table)?.as_table())
        .collect();
    // `[target.'cfg(…)'.dependencies]`
    if let Some(targets) = manifest.get("target").and_then(|t| t.as_table()) {
        for target in targets.values().filter_map(|t| t.as_table()) {
            sections.extend(
                tables
                    .iter()
                    .filter_map(|table| target.get(*table)?.as_table()),
            );
        }
    }
    for section in sections {
        for (name, dependency) in section {
            if let Some(path) = dependency.get("path").and_then(|path| path.as_str()) {
                path_deps.push(normalize(&format!("{dir}/{path}")));
            } else if dependency.get("workspace").and_then(toml::Value::as_bool) == Some(true)
                && let Some(path) = workspace.and_then(|deps| deps.get(name))
            {
                path_deps.push(path.clone());
            }
        }
    }
    Some(Package {
        dir: dir.to_owned(),
        name,
        kind: PackageKind::Cargo { path_deps },
    })
}

/// A Node package from its `package.json` in `dir`.
fn node_package(dir: &str, text: &str) -> Option<Package> {
    let manifest: serde_json::Value = serde_json::from_str(text).ok()?;
    let scripts = manifest
        .get("scripts")
        .and_then(|scripts| scripts.as_object())
        .map(|scripts| {
            SCRIPTS
                .iter()
                .filter(|script| scripts.contains_key(**script))
                .map(|script| (*script).to_owned())
                .collect()
        })
        .unwrap_or_default();
    let mut workspace_deps = Vec::new();
    for table in ["dependencies", "devDependencies", "peerDependencies"] {
        if let Some(deps) = manifest.get(table).and_then(|deps| deps.as_object()) {
            for (name, version) in deps {
                if version
                    .as_str()
                    .is_some_and(|v| v.starts_with("workspace:"))
                {
                    workspace_deps.push(name.clone());
                }
            }
        }
    }
    Some(Package {
        dir: dir.to_owned(),
        name: manifest
            .get("name")
            .and_then(|name| name.as_str())
            .map(str::to_owned),
        kind: PackageKind::Node {
            scripts,
            workspace_deps,
        },
    })
}

/// `path` with its `.` and `..` parts folded away, `/`-separated, without a leading `/`.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    parts.join("/")
}

/// Whether a changed `path` is the workspace's own: its root manifests, lockfiles and shared
/// configuration, which every package builds with.
fn workspace_wide(path: &str) -> bool {
    const ROOT_FILES: &[&str] = &[
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "pnpm-lock.yaml",
        "pnpm-workspace.yaml",
        "package-lock.json",
        "yarn.lock",
    ];
    if path.starts_with(".cargo/") {
        return true;
    }
    if path.contains('/') {
        return false;
    }
    ROOT_FILES.contains(&path)
        || path.starts_with("rust-toolchain")
        || (path.starts_with("tsconfig") && path.ends_with(".json"))
}

/// The checks a set of changes calls for while its work is under way: those of the packages
/// it changed, never more (SESSION-FIXES-PLAN.md §11). What a change reaches beyond them (the
/// packages that depend on it, the workspace's own files, a path no package holds) is the
/// session's full checks' to cover, once, after the work lands.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    /// Each changed package checked, and its checks: `(command, workdir)`, the workdir
    /// repository-relative ("" for the root).
    groups: Vec<Group>,
    /// What the full checks cover instead, each with why.
    left: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    label: String,
    checks: Vec<(String, String)>,
}

impl Plan {
    fn text(&self, changed: usize) -> String {
        if changed == 0 {
            return "Nothing changed since your base: there are no affected checks.".into();
        }
        let mut text = format!(
            "{changed} changed file{} since your base. ",
            if changed == 1 { "" } else { "s" }
        );
        if self.groups.is_empty() {
            text.push_str("None of the changes has checks of its own.");
        } else {
            text.push_str("The checks of the packages you changed:");
        }
        for group in &self.groups {
            text.push_str(&format!("\n{}:", group.label));
            for (command, workdir) in &group.checks {
                if workdir.is_empty() {
                    text.push_str(&format!("\n- `{command}`"));
                } else {
                    text.push_str(&format!("\n- `{command}` (workdir {workdir})"));
                }
            }
        }
        if !self.left.is_empty() {
            text.push_str(&format!(
                "\nNot listed: {}. The session's full checks cover those once the work lands, \
                 one run for everyone's work: don't run the whole suite, a workspace-wide \
                 check or an app build yourself.",
                self.left.join("; ")
            ));
        }
        if !self.groups.is_empty() {
            text.push_str(
                "\nRun each with run_check (`command`, and `workdir` where one is given). A \
                 check that already ran on the same files answers from the cache.",
            );
        }
        text
    }
}

/// The checks `changed` calls for, given the workspace's `packages`, its package manager and
/// the checks learned per package folder: each changed package's own, and what is left to the
/// full checks.
fn affected(
    changed: &[String],
    packages: &[Package],
    manager: PackageManager,
    learned: &BTreeMap<String, Vec<String>>,
) -> Plan {
    let owner = |path: &str| {
        packages
            .iter()
            .enumerate()
            .filter(|(_, package)| {
                !package.dir.is_empty()
                    && (path == package.dir || path.starts_with(&format!("{}/", package.dir)))
            })
            .max_by_key(|(_, package)| package.dir.len())
            .map(|(index, _)| index)
    };
    let mut left = Vec::new();
    let mut touched = BTreeSet::new();
    for path in changed {
        if workspace_wide(path) {
            left.push(format!("{path} (the workspace's own)"));
            continue;
        }
        // A folder can hold a crate and a Node package: both are touched.
        match owner(path) {
            Some(index) => {
                let dir = &packages[index].dir;
                touched.extend(
                    packages
                        .iter()
                        .enumerate()
                        .filter(|(_, package)| package.dir == *dir)
                        .map(|(index, _)| index),
                );
            }
            None => left.push(format!("{path} (no package holds it)")),
        }
    }
    // The packages that depend on a touched one.
    let dependents: BTreeSet<String> = (0..packages.len())
        .filter(|index| !touched.contains(index))
        .filter(|index| {
            touched
                .iter()
                .any(|used| packages[*used].used_by(&packages[*index]))
        })
        .map(|index| packages[index].folder())
        .collect();
    if !dependents.is_empty() {
        left.push(format!(
            "the packages that depend on yours ({})",
            dependents.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let mut seen = BTreeSet::new();
    let mut groups = Vec::new();
    for index in touched {
        let package = &packages[index];
        let checks: Vec<(String, String)> = package_checks(package, manager, learned, false)
            .into_iter()
            .filter(|check| seen.insert(check.clone()))
            .collect();
        if checks.is_empty() {
            left.push(format!(
                "{} (it has no checks of its own)",
                package.folder()
            ));
            continue;
        }
        groups.push(Group {
            label: format!(
                "{} ({}changed)",
                package.folder(),
                package
                    .name
                    .as_ref()
                    .map(|name| format!("{name}, "))
                    .unwrap_or_default()
            ),
            checks,
        });
    }
    Plan { groups, left }
}

/// A package's checks: those learned for its folder, its check scripts, and a crate's tests,
/// lints and formatting.
fn package_checks(
    package: &Package,
    manager: PackageManager,
    learned: &BTreeMap<String, Vec<String>>,
    builds: bool,
) -> Vec<(String, String)> {
    let mut checks: Vec<(String, String)> = learned
        .get(&package.folder())
        .into_iter()
        .flatten()
        .map(|command| (command.clone(), package.dir.clone()))
        .collect();
    match (&package.kind, &package.name) {
        (PackageKind::Cargo { .. }, Some(name)) => {
            checks.push((format!("cargo test -p {name}"), String::new()));
            checks.push((
                format!("cargo clippy -p {name} --all-targets -- -D warnings"),
                String::new(),
            ));
            checks.push(("cargo fmt --check".into(), String::new()));
        }
        (PackageKind::Node { scripts, .. }, name) => {
            // A build is the full checks' (§11), not a package's own check.
            for script in scripts.iter().filter(|script| builds || *script != "build") {
                checks.push(match (manager, name) {
                    (PackageManager::Pnpm, Some(name)) => {
                        (format!("pnpm --filter {name} {script}"), String::new())
                    }
                    _ => (manager.run(script), package.dir.clone()),
                });
            }
        }
        (PackageKind::Cargo { .. }, None) => {}
    }
    let mut seen = BTreeSet::new();
    checks.retain(|check| seen.insert(check.clone()));
    checks
}

/// Everything: the checks learned for the root, the root's own check scripts, each other
/// package's learned checks and the check scripts of each Node package the root has no
/// script of that name for (a root `test` is taken to run its packages' tests), and the
/// workspace-wide Cargo commands.
fn everything_group(
    packages: &[Package],
    manager: PackageManager,
    learned: &BTreeMap<String, Vec<String>>,
) -> Group {
    let mut checks: Vec<(String, String)> = learned
        .get(".")
        .into_iter()
        .flatten()
        .map(|command| (command.clone(), String::new()))
        .collect();
    let mut root_scripts: BTreeSet<&str> = BTreeSet::new();
    for package in packages.iter().filter(|package| package.dir.is_empty()) {
        if let PackageKind::Node { scripts, .. } = &package.kind {
            checks.extend(
                scripts
                    .iter()
                    .map(|script| (manager.run(script), String::new())),
            );
            root_scripts.extend(scripts.iter().map(String::as_str));
        }
    }
    for package in packages.iter().filter(|package| !package.dir.is_empty()) {
        checks.extend(
            learned
                .get(&package.folder())
                .into_iter()
                .flatten()
                .map(|command| (command.clone(), package.dir.clone())),
        );
        if let PackageKind::Node { scripts, .. } = &package.kind {
            let child = Package {
                kind: PackageKind::Node {
                    scripts: scripts
                        .iter()
                        .filter(|script| !root_scripts.contains(script.as_str()))
                        .cloned()
                        .collect(),
                    workspace_deps: Vec::new(),
                },
                ..package.clone()
            };
            checks.extend(package_checks(&child, manager, &BTreeMap::new(), true));
        }
    }
    if packages
        .iter()
        .any(|package| matches!(package.kind, PackageKind::Cargo { .. }))
    {
        checks.push(("cargo test --workspace".into(), String::new()));
        checks.push((
            "cargo clippy --workspace --all-targets -- -D warnings".into(),
            String::new(),
        ));
        checks.push(("cargo fmt --check".into(), String::new()));
    }
    let mut seen = BTreeSet::new();
    checks.retain(|check| seen.insert(check.clone()));
    Group {
        label: "the whole workspace".into(),
        checks,
    }
}

/// Scripts a project keeps its full checks in, repository-relative.
const FULL_CHECK_SCRIPTS: &[&str] = &[
    "tools/full-checks.sh",
    "scripts/full-checks.sh",
    "scripts/full-checks",
    "bin/full-checks",
];
/// Root scripts that run a project's whole verification.
const FULL_CHECK_RUNS: &[&str] = &["verify", "ci"];

/// The project's full checks, as it documents them (SESSION-FIXES-PLAN.md §11): its own
/// full-checks script, which knows its prerequisites, generated files and known exclusions;
/// else a root `verify` or `ci` script; else every package's checks and the workspace-wide
/// Cargo commands. Each is `(command, workdir)`, run from the repository's root.
fn full_checks(
    root: &Path,
    packages: &[Package],
    manager: PackageManager,
    learned: &BTreeMap<String, Vec<String>>,
) -> Vec<(String, String)> {
    for script in FULL_CHECK_SCRIPTS {
        let path = root.join(script);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        #[cfg(unix)]
        let runs_itself = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let runs_itself = false;
        let command = if runs_itself {
            (*script).to_owned()
        } else {
            format!("bash {script}")
        };
        return vec![(command, String::new())];
    }
    let root_scripts = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|manifest| manifest.get("scripts").cloned());
    if let Some(run) = FULL_CHECK_RUNS.iter().find(|run| {
        root_scripts
            .as_ref()
            .is_some_and(|scripts| scripts.get(**run).is_some())
    }) {
        return vec![(manager.run(run), String::new())];
    }
    everything_group(packages, manager, learned).checks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_in(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Checks",
                "-c",
                "user.email=checks@example.com",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("brigadier-checks-{name}-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.canonicalize().unwrap())
        }
    }

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A repository that ignores logs, `.env*` files and a secret file.
    fn repo(name: &str) -> Temp {
        let dir = Temp::new(name);
        git_in(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("README.md"), "# Checks\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "*.log\n.env*\nsecret.json\n").unwrap();
        git_in(&dir, &["add", "-A"]);
        git_in(&dir, &["commit", "-q", "-m", "Start"]);
        dir
    }

    /// What the key holds of a checkout besides the command and toolchains.
    fn material(root: &Path) -> (String, Vec<String>) {
        let git = brigadier_git::Git::new(PathBuf::from("git"), std::env::vars_os().collect());
        let tree = git.open_worktree(root).unwrap().checkout_tree().unwrap();
        let packages = package_dirs(root).unwrap();
        (
            tree.0,
            input_files(root, root, &packages, &["secret.json".into()]),
        )
    }

    #[test]
    fn only_prefixes_that_change_nothing_leave_a_check_command() {
        use brigadier_sandbox::limits::{OpenFiles, UNLIMITED};
        let tmp = Path::new("/tmp/brigadier-test-1234abcd");
        // The daemon's limits: soft raised, hard unlimited (macOS).
        let limits = Some(OpenFiles {
            soft: 65_536,
            hard: UNLIMITED,
        });
        let same = |command: &'static str| canonical_command(command, tmp, limits);
        // Dropped: a TMPDIR the check sets anyway, a limit already in place.
        assert_eq!(
            same("TMPDIR=/tmp/brigadier-test-1234abcd cargo test"),
            "cargo test"
        );
        assert_eq!(
            same("export TMPDIR='/tmp/brigadier-test-1234abcd/'; cargo test"),
            "cargo test"
        );
        assert_eq!(same("ulimit -Sn 65536; cargo test"), "cargo test");
        assert_eq!(same("ulimit -S -n 65536 && cargo test"), "cargo test");
        assert_eq!(same("ulimit -Hn unlimited\ncargo test"), "cargo test");
        assert_eq!(
            same("ulimit -Sn 65536; TMPDIR=/tmp/brigadier-test-1234abcd cargo test -p core"),
            "cargo test -p core"
        );
        // Kept: a limit that lowers the soft one, sets the hard one (bash's `ulimit -n N`
        // sets both), or a folder the check doesn't set.
        for command in [
            "ulimit -n 8192; cargo test",
            "ulimit -n 65536; cargo test",
            "ulimit -Sn 8192; cargo test",
            "ulimit -Hn 65536; cargo test",
            "ulimit -n 8192 cargo test",
            "ulimit -u 100; cargo test",
            "TMPDIR=/tmp/elsewhere cargo test",
            "export TMPDIR=/tmp/brigadier-test-1234abcd",
            "FOO=1 cargo test",
        ] {
            assert_eq!(same(command), command, "{command}");
        }
        // `ulimit -n N` drops only where N is both limits.
        let equal = Some(OpenFiles {
            soft: 8192,
            hard: 8192,
        });
        assert_eq!(
            canonical_command("ulimit -n 8192; cargo test", tmp, equal),
            "cargo test"
        );
        // Unreadable limits: no ulimit is dropped.
        assert_eq!(
            canonical_command("ulimit -Sn 65536; cargo test", tmp, None),
            "ulimit -Sn 65536; cargo test"
        );
    }

    #[test]
    fn the_key_follows_edits_and_new_files_but_not_ignored_ones_except_secrets() {
        let root = repo("key");
        let clean = material(&root);
        assert_eq!(material(&root), clean, "stable");
        std::fs::write(root.join("debug.log"), "ignored\n").unwrap();
        assert_eq!(material(&root), clean, "an ignored file is not in it");
        std::fs::write(root.join("README.md"), "# Edited\n").unwrap();
        let edited = material(&root);
        assert_ne!(edited.0, clean.0, "an uncommitted edit is");
        std::fs::write(root.join("new.rs"), "fn main() {}\n").unwrap();
        let added = material(&root);
        assert_ne!(added.0, edited.0, "an untracked file is");
        std::fs::write(root.join(".env.local"), "KEY=1\n").unwrap();
        let env = material(&root);
        assert_eq!(env.0, added.0);
        assert_ne!(env.1, added.1, "an ignored env file is");
        std::fs::write(root.join("secret.json"), "{}\n").unwrap();
        let secret = material(&root);
        assert_ne!(secret.1, env.1, "a secret file is");
        std::fs::write(root.join("Cargo.lock"), "# lock\n").unwrap();
        let locked = material(&root);
        assert_ne!(locked, secret, "a lockfile is");
        std::fs::write(
            root.join(".gitignore"),
            "*.log\n.env*\nsecret.json\n.npmrc\n",
        )
        .unwrap();
        let ignoring = material(&root);
        std::fs::write(root.join(".npmrc"), "registry=https://example.invalid/\n").unwrap();
        let config = material(&root);
        assert_eq!(config.0, ignoring.0, "an ignored config isn't in the tree");
        assert_ne!(config.1, ignoring.1, "but it is in the key");
        // A package's own ignored env file, which a check run from the root may build with.
        std::fs::create_dir_all(root.join("apps/web")).unwrap();
        std::fs::write(root.join("apps/web/package.json"), "{\"name\": \"web\"}\n").unwrap();
        let package = material(&root);
        std::fs::write(root.join("apps/web/.env.production"), "API=1\n").unwrap();
        let env = material(&root);
        assert_eq!(env.0, package.0, "ignored, so not in the tree");
        assert_ne!(env.1, package.1, "a package's env file is");
        std::fs::write(root.join("apps/web/.env.production"), "API=2\n").unwrap();
        assert_ne!(material(&root).1, env.1, "and its content");
    }

    #[test]
    fn a_folder_outside_git_has_no_root() {
        let dir = Temp::new("none");
        assert_eq!(repo_root(&dir), None);
        let root = repo("root");
        std::fs::create_dir_all(root.join("crates/a")).unwrap();
        assert_eq!(repo_root(&root.join("crates/a")), Some(root.to_path_buf()));
        std::fs::write(
            root.join("crates/a/Cargo.toml"),
            "[package]\nname = \"a\"\n",
        )
        .unwrap();
        assert_eq!(package_folder(&root, &root.join("crates/a")), "crates/a");
        assert_eq!(package_folder(&root, &root), ".");
    }

    #[test]
    fn a_read_only_codex_worker_checks_from_its_scratch_folder() {
        let root = repo("grant");
        let (tree, scratch) = (root.join("tree"), root.join("scratch"));
        std::fs::create_dir_all(&tree).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        // Codex can't start in a read-only folder: its access grants its scratch folder.
        let grant = cwd_grant(ProviderKind::Codex, false, Some(&tree), &scratch);
        assert_eq!(grant.as_deref(), Some(scratch.as_path()));
        assert_eq!(check_root(Some(&tree), grant.as_deref(), &tree), scratch);
        // So the checkout isn't writable in its checks' sandbox, and their failures there
        // aren't anyone else's answer.
        let access = Access::Scoped {
            write_cwd: true,
            writable_roots: Vec::new(),
            network: true,
            deny_read: Vec::new(),
            unix_sockets: Vec::new(),
        };
        assert_eq!(class_of(&access, Some(&tree), true), "sandboxed network");
        assert_eq!(
            class_of(&access, Some(&tree), false),
            "sandboxed network writes-checkout"
        );
        // A writer, and any Claude worker, have their access made for their checkout.
        for (provider, writes) in [
            (ProviderKind::Codex, true),
            (ProviderKind::Claude, false),
            (ProviderKind::Claude, true),
        ] {
            let grant = cwd_grant(provider, writes, Some(&tree), &scratch);
            assert_eq!(grant, None, "{provider:?} {writes}");
            assert_eq!(check_root(Some(&tree), None, &tree.join("sub")), tree);
        }
        assert_eq!(class_of(&Access::Full, Some(&tree), false), "full");
    }

    #[test]
    fn a_checks_output_hides_the_projects_secrets() {
        let redactor = crate::manager::secrets::redactor(vec!["s3cret-value-123".into()]);
        let output = redact(
            b"token=s3cret-value-123 ok\n".to_vec(),
            &[None, redactor.clone()],
        );
        let text = String::from_utf8(output).unwrap();
        assert!(!text.contains("s3cret-value-123"), "{text}");
        assert!(text.ends_with(" ok\n"), "{text}");
        // Output that isn't text is kept byte for byte.
        let binary = vec![0xff, 0xfe, b'a'];
        assert_eq!(redact(binary.clone(), &[redactor]), binary);
    }

    #[test]
    fn a_command_the_shell_could_not_find_or_run_is_not_kept() {
        assert!(keepable("exit 0"));
        assert!(keepable("exit 1"));
        assert!(keepable("exit 2"));
        assert!(!keepable("exit 126"));
        assert!(!keepable("exit 127"));
    }

    fn kept(status: &str, task: Option<&str>, access: Option<&str>) -> Kept {
        Kept {
            status: status.into(),
            reply: "[exit 0]\nok\n".into(),
            wrapped: false,
            conversation_id: ConversationId("c1".into()),
            task_id: task.map(|task| TaskId(task.into())),
            access: access.map(str::to_owned),
            alias: "out-1".into(),
            blob: "b".into(),
            duration_ms: 1,
            ran_at_ms: 0,
        }
    }

    #[test]
    fn a_kept_result_is_each_callers_own_and_a_failure_only_under_the_same_sandbox() {
        let conversation = ConversationId("c1".into());
        let (t1, t2) = (TaskId("t1".into()), TaskId("t2".into()));
        let pass = kept("exit 0", Some("t1"), Some("full"));
        // The worker that ran it reads its own reply; another worker and the thread get theirs.
        assert_eq!(
            reuse_for(&pass, &conversation, Some(&t1), false, "full"),
            Some(true)
        );
        assert_eq!(
            reuse_for(&pass, &conversation, Some(&t2), false, "full"),
            Some(false)
        );
        assert_eq!(
            reuse_for(&pass, &conversation, None, false, "full"),
            Some(false)
        );
        // A pass answers under any sandbox.
        assert_eq!(
            reuse_for(&pass, &conversation, Some(&t1), false, "sandboxed"),
            Some(true)
        );
        // A failure answers only a caller its sandbox allowed the same; an old one, none.
        let failed = kept("exit 1", None, Some("sandboxed"));
        assert_eq!(
            reuse_for(&failed, &conversation, None, false, "sandboxed"),
            Some(true)
        );
        assert_eq!(
            reuse_for(&failed, &conversation, None, false, "sandboxed network"),
            None
        );
        assert_eq!(reuse_for(&failed, &conversation, None, false, "full"), None);
        assert_eq!(
            reuse_for(
                &kept("exit 1", None, None),
                &conversation,
                None,
                false,
                "full"
            ),
            None
        );
        // A result kept before the caller fields existed reads as the thread's.
        let old: Kept = serde_json::from_str(
            r#"{"status":"exit 0","reply":"r","wrapped":false,"conversationId":"c1","alias":"out-1","blob":"b","durationMs":1,"ranAtMs":0}"#,
        )
        .unwrap();
        assert_eq!(
            reuse_for(&old, &conversation, None, false, "full"),
            Some(true)
        );
    }

    #[tokio::test]
    async fn one_check_of_a_key_runs_at_a_time() {
        let key = format!("key-{}", uuid::Uuid::new_v4());
        let first = check_lock(&key, Duration::from_secs(5))
            .await
            .expect("free");
        // The same key waits for it; another key doesn't.
        assert!(check_lock(&key, Duration::from_millis(100)).await.is_none());
        let other = format!("{key}-other");
        assert!(
            check_lock(&other, Duration::from_millis(100))
                .await
                .is_some()
        );
        let waiter = tokio::spawn({
            let key = key.clone();
            async move { check_lock(&key, Duration::from_secs(5)).await.is_some() }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(first);
        assert!(waiter.await.unwrap(), "it runs once the other ended");
    }

    #[test]
    fn a_toolchain_is_used_by_its_words_only() {
        assert!(uses("cargo test -p core", &["cargo"]));
        assert!(uses("cd app && pnpm test", &["pnpm"]));
        assert!(!uses("make cargo-free", &["cargo"]));
        assert!(!uses("echo hi", &["node", "npm"]));
        assert_eq!(quote(Path::new("/a b/it's")), r"'/a b/it'\''s'");
    }

    #[test]
    fn learned_checks_read_back_as_written() {
        let commands = vec!["cargo test -p a".to_owned(), "pnpm run lint".to_owned()];
        assert_eq!(learned_commands(&learned_body(&commands)), commands);
    }

    /// `crates/index` used by `crates/core`, used by `crates/daemon`; `crates/other` alone; a
    /// pnpm workspace with `apps/desktop` using `packages/ui`.
    fn workspace() -> Vec<Package> {
        vec![
            cargo_package("crates/index", "[package]\nname = \"brigadier-index\"\n").unwrap(),
            cargo_package(
                "crates/core",
                "[package]\nname = \"brigadier-core\"\n[dependencies]\nbrigadier-index = { path = \"../index\" }\n",
            )
            .unwrap(),
            cargo_package(
                "crates/daemon",
                "[package]\nname = \"brigadierd\"\n[dev-dependencies.brigadier-core]\npath = \"../core\"\n",
            )
            .unwrap(),
            cargo_package("crates/other", "[package]\nname = \"other\"\n").unwrap(),
            node_package(
                "packages/ui",
                r#"{"name": "@b/ui", "scripts": {"test": "vitest", "dev": "vite"}}"#,
            )
            .unwrap(),
            node_package(
                "apps/desktop",
                r#"{"name": "desktop", "scripts": {"typecheck": "tsc", "build": "vite build"},
                    "dependencies": {"@b/ui": "workspace:*", "react": "^19"}}"#,
            )
            .unwrap(),
            node_package("", r#"{"name": "root", "scripts": {"lint": "eslint .", "start": "x"}}"#)
                .unwrap(),
        ]
    }

    fn commands(plan: &Plan) -> Vec<String> {
        plan.groups
            .iter()
            .flat_map(|group| &group.checks)
            .map(|(command, workdir)| {
                if workdir.is_empty() {
                    command.clone()
                } else {
                    format!("{command} @ {workdir}")
                }
            })
            .collect()
    }

    #[test]
    fn a_crates_workspace_dependencies_are_its_dependencies() {
        let root = repo("cargo-ws");
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\nbrigadier-index = { path = \"crates/index\" }\nserde = \"1\"\n",
        );
        write(
            "crates/index/Cargo.toml",
            "[package]\nname = \"brigadier-index\"\n",
        );
        write(
            "crates/core/Cargo.toml",
            "[package]\nname = \"brigadier-core\"\n[dependencies]\nbrigadier-index.workspace = true\nserde.workspace = true\n",
        );
        write(
            "crates/daemon/Cargo.toml",
            "[package]\nname = \"brigadierd\"\n[dependencies]\nbrigadier-core = { path = \"../core\" }\n",
        );
        write("crates/index/src/lib.rs", "pub fn index() {}\n");
        let files = listed_files(&root).unwrap();
        let packages = read_packages(&root, &files);
        let plan = affected(
            &["crates/index/src/lib.rs".into()],
            &packages,
            PackageManager::Npm,
            &BTreeMap::new(),
        );
        // The crate that uses it through the workspace's table is its dependent.
        assert!(commands(&plan).contains(&"cargo test -p brigadier-index".to_owned()));
        assert_eq!(
            plan.left,
            ["the packages that depend on yours (crates/core)"]
        );
    }

    #[test]
    fn a_workspace_manifest_is_not_a_package() {
        assert_eq!(
            cargo_package("", "[workspace]\nmembers = [\"crates/*\"]\n"),
            None
        );
    }

    #[test]
    fn a_change_lists_only_its_own_packages_checks() {
        let learned = BTreeMap::from([(
            "crates/core".to_owned(),
            vec!["cargo test -p brigadier-core --features slow".to_owned()],
        )]);
        let plan = affected(
            &["crates/index/src/lib.rs".into()],
            &workspace(),
            PackageManager::Pnpm,
            &learned,
        );
        assert_eq!(
            commands(&plan),
            [
                "cargo test -p brigadier-index",
                "cargo clippy -p brigadier-index --all-targets -- -D warnings",
                "cargo fmt --check",
            ]
        );
        // What depends on it is the full checks', named but not listed.
        assert_eq!(
            plan.left,
            ["the packages that depend on yours (crates/core)"]
        );
        let text = plan.text(1);
        assert!(text.contains("`cargo test -p brigadier-index`"), "{text}");
        assert!(!text.contains("brigadier-core"), "{text}");
        assert!(text.contains("don't run the whole suite"), "{text}");
        // A Node package's dependents through `workspace:` aren't listed either.
        let plan = affected(
            &["packages/ui/src/button.tsx".into()],
            &workspace(),
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert_eq!(commands(&plan), ["pnpm --filter @b/ui test"]);
        assert_eq!(
            plan.left,
            ["the packages that depend on yours (apps/desktop)"]
        );
        // Without pnpm, the script runs in its folder; a build is never a package's check.
        let plan = affected(
            &["apps/desktop/src/main.ts".into()],
            &workspace(),
            PackageManager::Npm,
            &BTreeMap::new(),
        );
        assert_eq!(commands(&plan), ["npm run typecheck @ apps/desktop"]);
        assert!(plan.left.is_empty());
        // Two changed packages that share a check list it once.
        let plan = affected(
            &[
                "crates/index/src/lib.rs".into(),
                "crates/other/src/lib.rs".into(),
            ],
            &workspace(),
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        let listed = commands(&plan);
        assert_eq!(
            listed.iter().filter(|c| *c == "cargo fmt --check").count(),
            1,
            "{listed:?}"
        );
    }

    #[test]
    fn a_root_lockfile_or_config_change_is_left_to_the_full_checks() {
        for path in [
            "Cargo.lock",
            "Cargo.toml",
            "pnpm-lock.yaml",
            "tsconfig.base.json",
            ".cargo/config.toml",
            "rust-toolchain.toml",
        ] {
            let plan = affected(
                &["crates/index/src/lib.rs".into(), path.into()],
                &workspace(),
                PackageManager::Pnpm,
                &BTreeMap::from([(".".to_owned(), vec!["make check".to_owned()])]),
            );
            assert_eq!(
                commands(&plan),
                [
                    "cargo test -p brigadier-index",
                    "cargo clippy -p brigadier-index --all-targets -- -D warnings",
                    "cargo fmt --check",
                ],
                "{path}"
            );
            assert!(
                plan.left.iter().any(|left| left.contains(path)),
                "{path}: {:?}",
                plan.left
            );
            let text = plan.text(2);
            assert!(!text.contains("--workspace"), "{text}");
            assert!(!text.contains("make check"), "{text}");
        }
        // Alone, it lists nothing at all.
        let plan = affected(
            &["pnpm-lock.yaml".into()],
            &workspace(),
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert!(plan.groups.is_empty());
        let text = plan.text(1);
        assert!(
            text.contains("None of the changes has checks of its own"),
            "{text}"
        );
        assert!(!text.contains("run_check"), "{text}");
        // A nested manifest is its package's.
        let plan = affected(
            &["packages/ui/package.json".into()],
            &workspace(),
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert_eq!(commands(&plan), ["pnpm --filter @b/ui test"]);
    }

    #[test]
    fn a_changed_package_with_no_checks_of_its_own_is_left_to_the_full_checks() {
        let mut packages = workspace();
        packages.push(node_package("packages/icons", r#"{"name": "@b/icons"}"#).unwrap());
        let plan = affected(
            &["packages/icons/index.ts".into()],
            &packages,
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert!(plan.groups.is_empty());
        assert_eq!(plan.left, ["packages/icons (it has no checks of its own)"]);
        // Once a check is learned for it, that is its check.
        let plan = affected(
            &["packages/icons/index.ts".into()],
            &packages,
            PackageManager::Pnpm,
            &BTreeMap::from([(
                "packages/icons".to_owned(),
                vec!["pnpm run svgo".to_owned()],
            )]),
        );
        assert!(plan.left.is_empty());
        assert_eq!(commands(&plan), ["pnpm run svgo @ packages/icons"]);
    }

    #[test]
    fn a_path_no_package_holds_is_left_to_the_full_checks() {
        let plan = affected(
            &["docs/THREAD-PLAN.md".into()],
            &workspace(),
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert!(plan.groups.is_empty());
        assert_eq!(plan.left, ["docs/THREAD-PLAN.md (no package holds it)"]);
        assert!(!plan.text(1).contains("--workspace"));
        assert_eq!(
            affected(&[], &workspace(), PackageManager::Pnpm, &BTreeMap::new()).text(0),
            "Nothing changed since your base: there are no affected checks."
        );
    }

    #[test]
    fn the_full_checks_are_the_projects_own_script_else_its_verify_else_everything() {
        let root = std::env::temp_dir().join(format!(
            "brigadier-full-checks-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(root.join("tools")).unwrap();
        let full = |root: &Path| {
            full_checks(root, &workspace(), PackageManager::Pnpm, &BTreeMap::new())
                .into_iter()
                .map(|(command, _)| command)
                .collect::<Vec<_>>()
        };
        // Nothing documented: every package's checks and the workspace-wide ones.
        let everything = full(&root);
        assert!(
            everything.contains(&"cargo test --workspace".to_owned()),
            "{everything:?}"
        );
        assert!(
            everything.contains(&"pnpm --filter desktop build".to_owned()),
            "{everything:?}"
        );
        // A root `verify` (or `ci`) script runs the whole verification.
        std::fs::write(
            root.join("package.json"),
            r#"{"scripts": {"ci": "x", "verify": "y"}}"#,
        )
        .unwrap();
        assert_eq!(full(&root), ["pnpm run verify"]);
        // The project's full-checks script wins, run by bash unless it runs itself.
        let script = root.join("tools/full-checks.sh");
        std::fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(full(&root), ["bash tools/full-checks.sh"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(full(&root), ["tools/full-checks.sh"]);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Runs `run_check`'s commit lines in `tree` as a check would, then reads its record.
    fn commit_in(tree: &Path, message: &str) -> String {
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(commit_command(message))
            .current_dir(tree)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        commit_record(
            &brigadier_git::Git::new("git".into(), Vec::new()),
            tree,
            message,
        )
    }

    /// A repository `run_check`'s commit can commit in, with its own user.
    fn repo_at(dir: &Path) {
        git_in(dir, &["init", "-q"]);
        git_in(dir, &["config", "user.email", "checks@example.com"]);
        git_in(dir, &["config", "user.name", "Checks"]);
    }

    /// A commit's line says what it holds and, from git status after it, what it left: an
    /// ignored file is named nowhere, a change `git add -A` couldn't take is named.
    #[test]
    fn a_commit_says_what_it_holds_and_what_git_status_still_shows() {
        let temp = Temp::new("commit-record");
        let tree = temp.0.as_path();
        repo_at(tree);
        std::fs::write(tree.join(".gitignore"), "*.log\n").unwrap();
        std::fs::write(tree.join("a.txt"), "a\n").unwrap();
        std::fs::write(tree.join("debug.log"), "scratch\n").unwrap();
        let line = commit_in(tree, "Add a");
        assert!(
            line.contains("\"Add a\" with your trailer: 2 files changed, 2 insertions(+) (.gitignore, a.txt). Nothing is left uncommitted."),
            "{line}"
        );

        // A nested repository's own change is something `git add -A` can't take.
        let nested = tree.join("vendor");
        std::fs::create_dir_all(&nested).unwrap();
        repo_at(&nested);
        std::fs::write(nested.join("v.txt"), "v\n").unwrap();
        git_in(&nested, &["add", "v.txt"]);
        git_in(&nested, &["commit", "-q", "-m", "v"]);
        let line = commit_in(tree, "Add vendor");
        assert!(line.contains("Nothing is left uncommitted."), "{line}");
        std::fs::write(nested.join("v.txt"), "changed\n").unwrap();
        std::fs::write(tree.join("c.txt"), "c\n").unwrap();
        let line = commit_in(tree, "Add c");
        assert!(
            line.contains("(c.txt). Left uncommitted: vendor."),
            "{line}"
        );
    }

    #[test]
    fn a_long_list_names_ten_and_counts_the_rest() {
        let paths: Vec<String> = (0..12).map(|n| format!("f{n}")).collect();
        assert_eq!(
            listed(&paths),
            "f0, f1, f2, f3, f4, f5, f6, f7, f8, f9 and 2 more"
        );
    }
}
