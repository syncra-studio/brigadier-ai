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

use brigadier_brain::{NewNode, NodeKind, Origin, Provenance};
use brigadier_providers::redact::Redactor;
use brigadier_providers::{Access, ProviderKind};
use serde::{Deserialize, Serialize};

use super::SessionManager;
use super::conversation::Cli;
use super::run::{RUN_TIMEOUT_DEFAULT, RUN_TIMEOUT_MAX, run_command, run_workdir, shell_command};
use super::{blocking, git_error};
use crate::digest::TRIM_ABOVE;
use crate::model::{ConversationId, DomainEvent, Environment, ProjectId, Setup};
use crate::tools::RunCheck;
use crate::work::{OutputSource, TaskId};
use crate::{Error, Result, now_ms};

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
const KEY_RECIPE: &str = "brigadier-check-1";

/// Who calls `run_check`, and where its commands run.
struct Caller {
    conversation_id: ConversationId,
    task_id: Option<TaskId>,
    provider: ProviderKind,
    access: Access,
    cli: Arc<Cli>,
    /// The checkout: a worker's worktree, the thread's workspace.
    tree: Option<PathBuf>,
    scratch: PathBuf,
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
}

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
    /// `run_check` for the thread of `id` (`task`: none) or the worker of `task`.
    pub(crate) async fn run_check_tool(
        &self,
        id: &ConversationId,
        task: Option<&TaskId>,
        args: RunCheck,
    ) -> Result<String> {
        let caller = self.check_caller(id, task).await?;
        let command = args
            .command
            .as_deref()
            .map(str::trim)
            .filter(|command| !command.is_empty());
        let Some(command) = command else {
            return self.affected_checks(&caller).await;
        };
        let workdir = run_workdir(
            args.workdir.as_deref(),
            caller.tree.as_deref(),
            &caller.scratch,
        )?;
        let timeout = args
            .timeout_secs
            .map_or(RUN_TIMEOUT_DEFAULT, Duration::from_secs)
            .clamp(Duration::from_secs(1), RUN_TIMEOUT_MAX);
        let key = self.check_key(&caller, &workdir, command).await;
        // One run of a key at a time: a caller that waited answers from what the other kept.
        let _running = match &key {
            Ok(key) => check_lock(&key.hash, timeout).await,
            Err(_) => None,
        };
        if let (Ok(key), false) = (&key, args.rerun)
            && let Some(reply) = self.cached_check(&caller, key, command).await
        {
            return Ok(reply);
        }
        self.run_check_command(&caller, &workdir, command, timeout, key)
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
            let (cli, access) = live
                .session_access()
                .await
                .ok_or_else(|| Error::Invalid("the task's worker isn't running".into()))?;
            let task = self.task_by_id(id, task_id).await?;
            let redactor = self.task_redactor(&task).await;
            let workspace = task
                .workspace
                .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
            let tree = workspace.worktree.map(PathBuf::from);
            let scratch = PathBuf::from(workspace.scratch);
            let cwd_grant = cwd_grant(cli.provider, task.kind.writes(), tree.as_deref(), &scratch);
            return Ok(Caller {
                conversation_id: id.clone(),
                task_id: Some(task_id.clone()),
                provider: cli.provider,
                access,
                cli,
                tree,
                scratch,
                cwd_grant,
                redactor,
                base: workspace.base.map_or(Base::Head, Base::Commit),
                project,
                secret_files,
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
            cli,
            tree: launch.workspace.map(|workspace| workspace.path),
            scratch: self.owned_dir("orch", &id.0),
            cwd_grant: None,
            redactor,
            base,
            project,
            secret_files,
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
    async fn cached_check(&self, caller: &Caller, key: &Key, command: &str) -> Option<String> {
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
        Some(format!(
            "[cached: this command already ran on these same files {} ago; its result is below. \
             Pass rerun: true to run it again.]\n{reply}",
            ago(now_ms() - kept.ran_at_ms)
        ))
    }

    /// Runs `command` in `workdir` for `caller`, keeps its result under `key` (when there is
    /// one and the command ran to its end) and returns the reply.
    async fn run_check_command(
        &self,
        caller: &Caller,
        workdir: &Path,
        command: &str,
        timeout: Duration,
        key: std::result::Result<Key, String>,
    ) -> Result<String> {
        let spec = self.check_spec(caller, workdir, command)?;
        let started = Instant::now();
        let ran = run_command(
            self.runtime.platform().clone(),
            &spec,
            timeout,
            caller.cli.ended.clone(),
            &caller.cli.owner,
            self.runtime.ledger(),
        )
        .await?;
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
        if ran.status == "exit 0" {
            self.learn_check(caller, workdir, command).await;
        }
        Ok(reply)
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
        brigadier_providers::cli::apply_session_env(
            &mut spec,
            &super::workers::worker_env(&caller.scratch),
            &[],
        );
        // Workers run at low priority, and so do their checks.
        spec.low_priority = caller.task_id.is_some();
        Ok(spec)
    }

    /// Stores a check's whole output for `caller`'s conversation; returns the reply (the
    /// output itself up to [`TRIM_ABOVE`] bytes, else its digest) and the output's alias.
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
        let trimmed = output.len() > TRIM_ABOVE;
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

/// The checks a set of changes calls for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    /// Why everything is checked, when it is.
    everything: Option<String>,
    /// Each package checked, why, and its checks: `(command, workdir)`, the workdir
    /// repository-relative ("" for the root).
    groups: Vec<Group>,
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
        match &self.everything {
            Some(why) => text.push_str(&format!("Check everything: {why}.")),
            None => text.push_str("The affected checks, by package:"),
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
        text.push_str(
            "\nRun each with run_check (`command`, and `workdir` where one is given). A check \
             that already ran on the same files answers from the cache.",
        );
        text
    }
}

/// The checks `changed` calls for, given the workspace's `packages`, its package manager and
/// the checks learned per package folder.
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
    let mut everything = None;
    let mut touched = BTreeSet::new();
    for path in changed {
        if workspace_wide(path) {
            everything.get_or_insert_with(|| format!("{path} is the workspace's own"));
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
            None => {
                everything.get_or_insert_with(|| format!("{path} belongs to no package"));
            }
        }
    }
    // A changed package with no checks of its own (no check scripts, nothing learned) is
    // checked with everything rather than not at all.
    if everything.is_none()
        && let Some(index) = touched
            .iter()
            .find(|index| package_checks(&packages[**index], manager, learned).is_empty())
    {
        everything = Some(format!(
            "{} has no checks of its own",
            packages[*index].folder()
        ));
    }
    if let Some(why) = everything {
        return Plan {
            everything: Some(why),
            groups: vec![everything_group(packages, manager, learned)],
        };
    }
    // The packages that depend on a touched one, until none is added.
    let mut dependents = BTreeSet::new();
    loop {
        let reached: BTreeSet<usize> = touched.iter().chain(&dependents).copied().collect();
        let more: Vec<usize> = (0..packages.len())
            .filter(|index| !reached.contains(index))
            .filter(|index| {
                reached
                    .iter()
                    .any(|used| packages[*used].used_by(&packages[*index]))
            })
            .collect();
        if more.is_empty() {
            break;
        }
        dependents.extend(more);
    }
    let mut seen = BTreeSet::new();
    let mut groups = Vec::new();
    for (index, why) in touched.iter().map(|index| (*index, "changed")).chain(
        dependents
            .iter()
            .map(|index| (*index, "depends on a changed package")),
    ) {
        let package = &packages[index];
        let checks: Vec<(String, String)> = package_checks(package, manager, learned)
            .into_iter()
            .filter(|check| seen.insert(check.clone()))
            .collect();
        if checks.is_empty() {
            continue;
        }
        groups.push(Group {
            label: format!(
                "{} ({}{why})",
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
    Plan {
        everything: None,
        groups,
    }
}

/// A package's checks: those learned for its folder, its check scripts, and a crate's tests,
/// lints and formatting.
fn package_checks(
    package: &Package,
    manager: PackageManager,
    learned: &BTreeMap<String, Vec<String>>,
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
            for script in scripts {
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
            checks.extend(package_checks(&child, manager, &BTreeMap::new()));
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

    /// A repository that ignores logs, `.env*` files and a secret file.
    fn repo(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("brigadier-checks-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
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
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_folder_outside_git_has_no_root() {
        let dir =
            std::env::temp_dir().join(format!("brigadier-checks-none-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(repo_root(&dir), None);
        std::fs::remove_dir_all(&dir).unwrap();
        let root = repo("root");
        std::fs::create_dir_all(root.join("crates/a")).unwrap();
        assert_eq!(repo_root(&root.join("crates/a")), Some(root.clone()));
        std::fs::write(
            root.join("crates/a/Cargo.toml"),
            "[package]\nname = \"a\"\n",
        )
        .unwrap();
        assert_eq!(package_folder(&root, &root.join("crates/a")), "crates/a");
        assert_eq!(package_folder(&root, &root), ".");
        std::fs::remove_dir_all(&root).unwrap();
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
        std::fs::remove_dir_all(&root).unwrap();
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
        let commands = commands(&plan);
        for crate_name in ["brigadier-index", "brigadier-core", "brigadierd"] {
            assert!(
                commands.contains(&format!("cargo test -p {crate_name}")),
                "{crate_name}: {commands:?}"
            );
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_workspace_manifest_is_not_a_package() {
        assert_eq!(
            cargo_package("", "[workspace]\nmembers = [\"crates/*\"]\n"),
            None
        );
    }

    #[test]
    fn a_leaf_crate_change_checks_it_and_what_depends_on_it() {
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
        assert_eq!(plan.everything, None);
        assert_eq!(
            commands(&plan),
            [
                "cargo test -p brigadier-index",
                "cargo clippy -p brigadier-index --all-targets -- -D warnings",
                "cargo fmt --check",
                "cargo test -p brigadier-core --features slow @ crates/core",
                "cargo test -p brigadier-core",
                "cargo clippy -p brigadier-core --all-targets -- -D warnings",
                "cargo test -p brigadierd",
                "cargo clippy -p brigadierd --all-targets -- -D warnings",
            ]
        );
        assert!(
            plan.groups[1]
                .label
                .contains("depends on a changed package")
        );
        let text = plan.text(1);
        assert!(text.contains("`cargo test -p brigadierd`"), "{text}");
        assert!(!text.contains("other"), "{text}");
        // A Node package's dependents through `workspace:`, by pnpm filter.
        let plan = affected(
            &["packages/ui/src/button.tsx".into()],
            &workspace(),
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert_eq!(
            commands(&plan),
            [
                "pnpm --filter @b/ui test",
                "pnpm --filter desktop typecheck",
                "pnpm --filter desktop build",
            ]
        );
        // Without pnpm, the script runs in its folder.
        let plan = affected(
            &["apps/desktop/src/main.ts".into()],
            &workspace(),
            PackageManager::Npm,
            &BTreeMap::new(),
        );
        assert_eq!(
            commands(&plan),
            [
                "npm run typecheck @ apps/desktop",
                "npm run build @ apps/desktop"
            ]
        );
    }

    #[test]
    fn a_root_lockfile_or_config_change_checks_everything() {
        for path in [
            "Cargo.lock",
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
            assert!(plan.everything.as_deref().unwrap().contains(path), "{path}");
            assert_eq!(
                commands(&plan),
                [
                    "make check",
                    "pnpm run lint",
                    "pnpm --filter @b/ui test",
                    "pnpm --filter desktop typecheck",
                    "pnpm --filter desktop build",
                    "cargo test --workspace",
                    "cargo clippy --workspace --all-targets -- -D warnings",
                    "cargo fmt --check",
                ],
                "{path}"
            );
        }
        // A root script of the same name is taken to run its packages' (the root's `lint`
        // here), and a package's learned checks run too.
        let mut packages = workspace();
        packages.push(
            node_package(
                "packages/lint",
                r#"{"name": "@b/lint", "scripts": {"lint": "x"}}"#,
            )
            .unwrap(),
        );
        let plan = affected(
            &["pnpm-lock.yaml".into()],
            &packages,
            PackageManager::Pnpm,
            &BTreeMap::from([(
                "crates/core".to_owned(),
                vec!["cargo test -p brigadier-core --features slow".to_owned()],
            )]),
        );
        let commands = commands(&plan);
        assert!(
            !commands.contains(&"pnpm --filter @b/lint lint".to_owned()),
            "{commands:?}"
        );
        assert!(
            commands
                .contains(&"cargo test -p brigadier-core --features slow @ crates/core".to_owned()),
            "{commands:?}"
        );
        // A nested lockfile is its package's.
        let plan = affected(
            &["packages/ui/package.json".into()],
            &workspace(),
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert_eq!(plan.everything, None);
    }

    #[test]
    fn a_changed_package_with_no_checks_of_its_own_checks_everything() {
        let mut packages = workspace();
        packages.push(node_package("packages/icons", r#"{"name": "@b/icons"}"#).unwrap());
        let plan = affected(
            &["packages/icons/index.ts".into()],
            &packages,
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert!(
            plan.everything
                .as_deref()
                .unwrap()
                .contains("packages/icons has no checks of its own")
        );
        assert!(commands(&plan).contains(&"cargo test --workspace".to_owned()));
        // Once a check is learned for it, its own checks are enough.
        let plan = affected(
            &["packages/icons/index.ts".into()],
            &packages,
            PackageManager::Pnpm,
            &BTreeMap::from([(
                "packages/icons".to_owned(),
                vec!["pnpm run svgo".to_owned()],
            )]),
        );
        assert_eq!(plan.everything, None);
        assert_eq!(commands(&plan), ["pnpm run svgo @ packages/icons"]);
    }

    #[test]
    fn a_path_no_package_holds_checks_everything() {
        let plan = affected(
            &["docs/THREAD-PLAN.md".into()],
            &workspace(),
            PackageManager::Pnpm,
            &BTreeMap::new(),
        );
        assert!(
            plan.everything
                .as_deref()
                .unwrap()
                .contains("docs/THREAD-PLAN.md")
        );
        assert!(commands(&plan).contains(&"cargo test --workspace".to_owned()));
        assert_eq!(
            affected(&[], &workspace(), PackageManager::Pnpm, &BTreeMap::new()).text(0),
            "Nothing changed since your base: there are no affected checks."
        );
    }
}
