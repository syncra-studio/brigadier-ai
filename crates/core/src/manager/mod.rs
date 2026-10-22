//! The session manager: live sessions, Chats and their workers.
//!
//! It turns the conversation log into running CLI sessions and back:
//!
//! - one thread per Brigadier session ([`conversation`], [`thread`]): a CLI session that
//!   reads, runs and makes tiny edits in the session's workspace and calls the Brigadier MCP
//!   tools ([`tools`]);
//! - workers, one CLI session per task, in their own worktrees ([`workers`]), whose accepted
//!   work lands on the session's branch ([`landing`]);
//! - one CLI session per Chat, with no orchestrator and no workers;
//! - the cards the user answers ([`cards`]), the grants the sessions hold, and the lifecycle
//!   (hibernate, archive, restore, delete) with the cleanup of everything they create
//!   ([`lifecycle`]).
//!
//! Every CLI session is disposable: the conversation stream is the truth, and a session that
//! is gone (hibernated with its files cleaned up, archived, crashed) is started again from it.

mod accounts;
mod add_project;
mod app_call;
mod app_state;
// Nothing feeds it since `run` went (S6); the module goes in S12.
#[allow(dead_code)]
mod background_runs;
mod brain_first;
mod brain_jobs;
mod brains;
mod branch_names;
mod branches;
pub(crate) mod browser_tool;
mod cards;
mod ci;
mod clean_up;
mod cli;
mod cli_thread;
mod cli_tools;
mod close_tab;
mod closing;
mod codex_children;
mod cold;
mod computer_host;
mod open_in_app;
mod terminal_run;
pub use computer_host::ComputerNews;
mod computer_use;
mod constraints;
mod context_pack;
mod conversation;
pub(crate) mod decisions;
pub mod disk;
mod engine;
mod evidence;
mod export;
mod fallback;
#[cfg(debug_assertions)]
pub mod fault;
mod feedback;
mod files;
mod find_repos;
#[cfg(test)]
mod flow;
mod fork;
mod fresh_base;
mod git_actions;
mod inbox_store;
mod instructions;
pub mod integrations;
mod kept;
#[cfg(all(test, unix))]
mod kept_live_tests;
mod landing;
mod lifecycle;
mod local_files;
mod machine;
mod mcp_auth;
mod memory_guard;
mod outcomes;
mod outputs;
pub mod overnight;
mod page_tools;
mod past_projects;
mod phases;
mod plain_workers;
mod preview;
mod preview_ready;
mod project_removal;
mod project_sessions;
mod prompts;
mod protected_folders;
mod pull_request;
mod quiet;
pub(crate) mod reads;
mod rebirth;
mod refcheck;
mod requests;
mod reruns;
mod research;
mod review;
mod routing;
mod run;
pub mod search;
mod secrets;
mod session_dir;
mod setup_script;
mod side_chat;
mod sidecars;
mod source_actions;
mod takeover;
mod thread;
mod thread_images;
mod thread_items;
mod thread_metrics;
mod thread_scratch;
mod tool_output;
mod tools;
mod trust;
mod turn_changes;
mod uninstall;
pub(super) mod upgrade;
mod usage;
mod usage_view;
mod user_rebase;
pub mod warm;
mod watchdog;
mod worker_handoff;
mod worker_resume;
mod workers;
pub mod workspace;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use brigadier_git::Git;
use brigadier_providers::{BoxFuture, ProviderEvent, ProviderKind};
use tokio_util::task::TaskTracker;

use crate::model::{
    Conversation, ConversationId, ConversationKind, DomainEvent, Environment, EnvironmentRequest,
    Lifecycle, ModelChoice, ProjectId, Rating, RepoInfo, Setup, SetupRequest,
};
use crate::runtime::{Runtime, Spawner};
use crate::sessions::Origin;
use crate::tools::{Grants, Role, ToolCall, ToolHost, ToolReply};
use crate::work::{DiffStat, TaskId, WorkerDiff};
use crate::{Core, Error, Result};

pub(crate) use background_runs::background_run_number;
pub use brains::{BrainCounters, IndexRunStats};
pub use cli::CliCaller;
pub use closing::Turn;
pub use conversation::SendOutcome;
pub(crate) use conversation::interrupted_by_quit;
pub use instructions::{COMMAND_PATHS_MAX, command_paths, lexical};
pub use mcp_auth::McpSignInStatus;
pub use side_chat::BrowserPage;
pub use takeover::{HostedTerminal, TerminalHost, TerminalSnapshot, WorkerTerminal};
pub use tool_output::{HOOK_GRANT_ENV, HookOutput, OUTPUT_MAX_BYTES, STOP_GRANT_ENV};
pub use uninstall::TearDown;

use self::cards::Waiters;
use self::conversation::ConvLive;
use self::workers::TaskLive;

/// Where the session manager finds what it hands to CLI sessions.
#[derive(Debug, Clone)]
pub struct ManagerConfig {
    /// The running `brigadierd`: CLIs start `brigadierd mcp` as their Brigadier MCP server.
    pub daemon_exe: PathBuf,
}

pub struct SessionManager {
    me: Weak<SessionManager>,
    core: Arc<Core>,
    runtime: Arc<Runtime>,
    spawner: Spawner,
    config: ManagerConfig,
    git: Git,
    data_dir: PathBuf,
    grants: Grants,
    /// Sign-ins to remote MCP servers shared between the agents, and their proxy.
    mcp_auth: Arc<mcp_auth::McpAuth>,
    convs: Mutex<HashMap<ConversationId, Arc<ConvLive>>>,
    tasks: Mutex<HashMap<TaskId, Arc<TaskLive>>>,
    /// What plain workers' CLIs did that their sessions' `brigadier wait` hasn't taken yet.
    worker_events: plain_workers::WorkerEvents,
    /// The cards a `brigadier ask` waits on: their answers go back to it, not into the thread.
    cli_asks: cli_tools::CliAsks,
    /// Held while a new-worktree session's own worktree is created, so parallel first tasks
    /// create it once.
    session_worktrees: tokio::sync::Mutex<()>,
    /// Held while a session is let into a project's own checkout (who works there looked for,
    /// then the session recorded), so two can't both be let in.
    local_checkouts: tokio::sync::Mutex<()>,
    /// Held while a plan is proposed (checked against the plans as they are, then recorded
    /// and the open ones replaced) and while a review result decides a plan, so the two
    /// can't interleave.
    plans: tokio::sync::Mutex<()>,
    /// Held while an open uncommitted-changes question is looked for and one opened, so two
    /// delegations at once open one card (#179).
    uncommitted_asks: tokio::sync::Mutex<()>,
    /// Held while the thread's new commits are looked for and their tip recorded
    /// ([`thread`]), so a range is taken once.
    thread_scans: tokio::sync::Mutex<()>,
    turn_snapshots: turn_changes::TurnSnapshots,
    /// The context window each model last reported, by provider and the model asked for: a
    /// Claude session learns its own only when its first turn ends, and its overflow guard needs
    /// it before then (BUILD-PLAN D2.3, D2.3b).
    known_windows: std::sync::Mutex<HashMap<(ProviderKind, String), i64>>,
    /// The engine's branches and their tips, for the ref check (PLAN bug 6); read and written
    /// under each repository's ref lock, off the async threads.
    ref_records: Arc<refcheck::RefRecords>,
    /// Each checkout's Git Output, for the Source panel ([`source_actions`]).
    source_output: source_actions::SourceOutput,
    /// The thread's commands gone to the background ([`background_runs`]).
    background_runs: background_runs::BackgroundRuns,
    /// The previews running now ([`preview`]).
    previews: preview::Previews,
    /// When each ready session's pull request was last looked up at a connect
    /// (`refresh_pull_request_soon`).
    pr_looked_up: Mutex<HashMap<ConversationId, std::time::Instant>>,
    /// Send feedback's GitHub sign-in at work, if any.
    feedback_sign_in: Mutex<Option<feedback::GhSignIn>>,
    /// A stand-in `gh` for tests.
    #[cfg(test)]
    pub(crate) gh_override: Mutex<Option<ci::Gh>>,
    /// A stand-in `curl` for tests (Send feedback's reads of GitHub's public API).
    #[cfg(test)]
    pub(crate) curl_override: Mutex<Option<PathBuf>>,
    /// Held while a task is read, changed and recorded (`update_task`), so two writers can't
    /// each write back a copy that lacks the other's change.
    task_writes: tokio::sync::Mutex<()>,
    /// Held while "Waiting on you" items are read, added and resolved, so one is never added
    /// twice.
    waiting: tokio::sync::Mutex<()>,
    /// Tasks being stopped: a landing that fails meanwhile (its worktree going away under
    /// it) is no news for the orchestrator.
    stopping: Mutex<HashSet<TaskId>>,
    /// Held while a rerun one tier up is checked and made, so a task reruns once.
    reruns_starting: tokio::sync::Mutex<()>,
    /// Tasks the user stopped (a worker's Stop, Stop all, or the stop of a turn), from the
    /// moment the stop starts: the thread may no longer message, steer or approve them.
    user_stops: Mutex<HashSet<TaskId>>,
    waiters: Waiters,
    admitting: AtomicBool,
    background: TaskTracker,
    brains: brains::Brains,
    /// Development builds: usage limits armed to exercise fallback.
    #[cfg(debug_assertions)]
    faults: fault::Faults,
    research: research::Research,
    overnight: overnight::Runs,
    /// How long a worker's question waits before its safe default may stand in, in ms (an
    /// hour; tests shorten it).
    question_timeout_ms: std::sync::atomic::AtomicU64,
    /// Holds new work while the machine struggles, and runs heavy commands one at a time.
    machine: Arc<crate::machine::MachineWatch>,
    /// Conversations being archived or deleted: their fences and cleanups.
    closing: closing::Closing,
    /// The computer-use helper's copy in the data folder ([`computer_use`]).
    computer_use: computer_use::ComputerUse,
    /// The helper as Settings, the menu bar and the watchdog tend it ([`computer_host`]).
    computer_host: computer_host::ComputerHost,
    /// Agents' browser calls on their way to the app ([`browser_tool`]).
    app: Arc<app_call::AppHub>,
    browser: Arc<browser_tool::BrowserHub>,
    /// Where "Open in terminal" runs a worker's session (the daemon's terminals).
    terminal_host: std::sync::OnceLock<Arc<dyn takeover::TerminalHost>>,
    /// The user's pages, which a thread may read and change from the page's own composer.
    pages: std::sync::OnceLock<Arc<crate::pages::Pages>>,
    /// Tabs' own chats: whether they may run yet, and what browser tabs sent them.
    tab_chats: side_chat::TabChats,
    /// The checkouts a tab chat saved a file in, for every window ([`page_tools`]).
    checkout_saves: tokio::sync::broadcast::Sender<crate::work::WorkspaceTarget>,
    /// Each conversation's running turn as its CLI streams it ([`cli_thread`]).
    cli_threads: std::sync::Mutex<HashMap<ConversationId, crate::thread_view::LiveTurn>>,
    /// What live streams add to threads, for every window.
    thread_news: tokio::sync::broadcast::Sender<crate::thread_view::ThreadNews>,
    /// CLI session files read for threads.
    session_files: crate::thread_view::FileCache,
    /// Tests: the home whose CLI settings folder trust is written to. Without it, tests write
    /// none (never the real `~/.claude.json` or `~/.codex/config.toml`).
    #[cfg(test)]
    pub(crate) trust_home: std::sync::OnceLock<PathBuf>,
    /// Tests: what a landing runs once it has rewritten the task branch, before it moves
    /// the target.
    #[cfg(test)]
    pub(crate) landing_rewritten: std::sync::Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Background model turns, for maintenance's idle check.
    activity: Arc<quiet::Activity>,
    /// The subfolder rules each thread's CLI session was given by its output hook, by its
    /// hook's grant (BUILD-PLAN D1.6).
    rules_given: std::sync::Mutex<
        std::collections::HashMap<ConversationId, (String, std::collections::HashSet<String>)>,
    >,
    /// The MCP servers and skills of every agent's own configuration ([`integrations`]),
    /// read from the CLIs' environment. Tests read an empty home unless they set one.
    registry: std::sync::RwLock<Arc<brigadier_registry::Registry>>,
}

impl SessionManager {
    /// `reported`, the window a session of `model` just reported, kept for its model's later
    /// sessions; or, when it reported none, the one its model reported last. A session on the
    /// CLI's default model (none named) shares nothing: the default may differ from one session
    /// to the next.
    pub(crate) fn model_window(
        &self,
        provider: ProviderKind,
        model: Option<&str>,
        reported: Option<i64>,
    ) -> Option<i64> {
        let Some(model) = model else {
            return reported;
        };
        let mut known = self
            .known_windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let key = (provider, model.to_owned());
        match reported.filter(|window| *window > 0) {
            Some(window) => {
                known.insert(key, window);
                Some(window)
            }
            None => known.get(&key).copied(),
        }
    }

    /// Starts the manager. Conversations come back to life lazily, on their next message.
    pub async fn start(
        core: Arc<Core>,
        runtime: Arc<Runtime>,
        spawner: Spawner,
        config: ManagerConfig,
    ) -> Result<Arc<Self>> {
        let env = runtime.cli_env().clone();
        let git = Git::new(
            env.which("git").unwrap_or_else(|| PathBuf::from("git")),
            env.vars(),
        );
        {
            let git = git.clone();
            match tokio::task::spawn_blocking(move || git.version()).await {
                Ok(Ok(version)) => tracing::info!(%version, "git found"),
                Ok(Err(err)) => tracing::warn!(error = %err, "git is unusable; sessions will fail"),
                Err(err) => tracing::warn!(error = %err, "checking git failed"),
            }
        }
        let data_dir = runtime.platform().paths().data_dir.clone();
        {
            let data_dir = data_dir.clone();
            let _ = blocking(move || {
                outputs::clear_opened(&data_dir);
                Ok(())
            })
            .await;
        }
        let brains = brains::Brains::new(&data_dir);
        let machine = Arc::new(crate::machine::MachineWatch::new(
            runtime.platform().clone(),
        ));
        // A test's scripted session runs on a calm machine, whatever the host's heat or memory
        // pressure: a test that wants it strained says so.
        #[cfg(test)]
        if runtime.faked() {
            machine
                .guard
                .fake(brigadier_sandbox::MachineLoad::default());
        }
        let app = Arc::new(app_call::AppHub::default());
        let registry = {
            #[cfg(not(test))]
            let vars: Vec<(String, String)> = env
                .vars()
                .into_iter()
                .filter_map(|(name, value)| {
                    Some((name.into_string().ok()?, value.into_string().ok()?))
                })
                .collect();
            #[cfg(test)]
            let vars = vec![(
                "HOME".to_owned(),
                data_dir.join("integrations-home").display().to_string(),
            )];
            let registry = tokio::task::spawn_blocking(move || integrations::registry(vars))
                .await
                .map_err(|err| Error::Invalid(format!("reading the integrations: {err}")))?;
            #[cfg(not(test))]
            if let Err(err) = registry.watch() {
                tracing::warn!(error = %err, "the integrations' configs aren't watched");
            }
            registry
        };
        let manager = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            core,
            runtime,
            spawner,
            config,
            git,
            data_dir,
            registry: std::sync::RwLock::new(registry),
            grants: Grants::default(),
            mcp_auth: {
                let me = me.clone();
                Arc::new(mcp_auth::McpAuth::new(
                    // Tests never touch the user's Keychain.
                    #[cfg(not(test))]
                    mcp_auth::system_keychain(),
                    #[cfg(test)]
                    Arc::new(mcp_auth::Memory::default()),
                    Arc::new(move |grant: &str| {
                        me.upgrade()
                            .is_some_and(|manager| manager.grants.resolve(grant).is_some())
                    }),
                ))
            },
            convs: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            worker_events: Default::default(),
            cli_asks: Default::default(),
            session_worktrees: tokio::sync::Mutex::new(()),
            local_checkouts: tokio::sync::Mutex::new(()),
            plans: tokio::sync::Mutex::new(()),
            uncommitted_asks: tokio::sync::Mutex::new(()),
            thread_scans: tokio::sync::Mutex::new(()),
            turn_snapshots: turn_changes::TurnSnapshots::default(),
            known_windows: std::sync::Mutex::new(HashMap::new()),
            ref_records: Arc::default(),
            source_output: source_actions::SourceOutput::default(),
            background_runs: background_runs::BackgroundRuns::default(),
            previews: preview::Previews::default(),
            pr_looked_up: Mutex::default(),
            #[cfg(test)]
            gh_override: Mutex::default(),
            #[cfg(test)]
            curl_override: Mutex::default(),
            feedback_sign_in: Mutex::default(),
            task_writes: tokio::sync::Mutex::new(()),
            waiting: tokio::sync::Mutex::new(()),
            stopping: Mutex::new(HashSet::new()),
            reruns_starting: tokio::sync::Mutex::new(()),
            user_stops: Mutex::new(HashSet::new()),
            waiters: Waiters::default(),
            admitting: AtomicBool::new(true),
            background: TaskTracker::new(),
            brains,
            #[cfg(debug_assertions)]
            faults: fault::Faults::default(),
            research: research::Research::default(),
            overnight: overnight::Runs::default(),
            question_timeout_ms: std::sync::atomic::AtomicU64::new(
                workers::QUESTION_TIMEOUT.as_millis() as u64,
            ),
            machine,
            closing: closing::Closing::default(),
            computer_use: computer_use::ComputerUse::default(),
            computer_host: computer_host::ComputerHost::default(),
            browser: Arc::new(browser_tool::BrowserHub::new(app.clone())),
            app,
            terminal_host: std::sync::OnceLock::new(),
            pages: std::sync::OnceLock::new(),
            tab_chats: side_chat::TabChats::default(),
            checkout_saves: tokio::sync::broadcast::channel(16).0,
            cli_threads: std::sync::Mutex::default(),
            thread_news: tokio::sync::broadcast::channel(1024).0,
            session_files: crate::thread_view::FileCache::default(),
            #[cfg(test)]
            trust_home: std::sync::OnceLock::new(),
            #[cfg(test)]
            landing_rewritten: std::sync::Mutex::new(None),
            activity: Arc::default(),
            rules_given: std::sync::Mutex::default(),
        });
        manager.install_worktree_remover();
        // Each step's future is polled from the heap: made in this frame, they took half a
        // MiB of an unoptimized build's stack at once.
        crate::heap(|| manager.start_machine_watch()).await;
        // Cleanups a quit cut off finish before anything of those conversations resumes.
        crate::heap(|| manager.finish_cut_off_cleanups()).await;
        // The thread engine's first start deletes the earlier engine's conversations, before
        // anything of theirs could resume.
        crate::heap(|| manager.switch_engine()).await?;
        // Sessions open at the strip-down's first start are archived, not carried over.
        crate::heap(|| manager.archive_open_at_upgrade()).await;
        // Before any session's instructions are written, which carry the preference.
        crate::heap(|| manager.carry_short_replies()).await;
        crate::heap(|| manager.recover_active_runs()).await;
        crate::heap(|| manager.recover()).await;
        crate::heap(|| manager.resume_runs()).await;
        manager.start_overnight_clock();
        crate::heap(|| manager.open_brains()).await;
        manager.start_hibernation_timer();
        manager.start_watchdog();
        manager.start_computer_watchdog();
        manager.retry_waiting_on_provider_checks();
        manager.start_checkpoint_timer();
        Ok(manager)
    }

    /// Grants held by live CLI sessions.
    pub fn grants(&self) -> &Grants {
        &self.grants
    }

    pub fn core(&self) -> &Arc<Core> {
        &self.core
    }

    /// Ends every live CLI session (their work stays in the log, ready to continue).
    /// Tests: the daemon dies here: nothing new is admitted and nothing more is written.
    #[cfg(test)]
    pub(crate) async fn halt(&self) {
        self.admitting.store(false, Ordering::Release);
        let _ = self.core.store().shutdown().await;
    }

    pub async fn shutdown(&self) {
        self.admitting.store(false, Ordering::Release);
        // Previews end with Brigadier (their ends recorded while the store is up).
        self.stop_all_previews().await;
        // Archives under way finish first (bounded) while the providers and the store are up.
        self.finish_cleanups_for_quit().await;
        self.runtime.registry().cancel_refresh();
        self.research.stop.cancel();
        self.research.jobs.close();
        self.research.jobs.wait().await;
        let tasks: Vec<Arc<TaskLive>> = self.tasks_lock().values().cloned().collect();
        let convs: Vec<Arc<ConvLive>> = self.convs_lock().values().cloned().collect();
        let mut closing = tokio::task::JoinSet::new();
        for task in tasks {
            closing.spawn(async move { task.close_cli().await });
        }
        for conv in convs {
            closing.spawn(async move { conv.close_cli().await });
        }
        closing.join_all().await;
        brains::stop_watchers(self.brains.shutdown()).await;
        self.background.close();
    }

    /// The files of a session's checkout (its worktree, or the user's checkout), for the
    /// composer's @-mentions: at most [`MENTION_FILES`], and whether there were more; with
    /// `query`, only those matching it. A Chat has none.
    pub async fn list_files(
        &self,
        id: &ConversationId,
        query: Option<String>,
    ) -> Result<(Vec<String>, bool)> {
        let Some(Setup::Session {
            repo, environment, ..
        }) = self.core.conversation(id)?.setup
        else {
            return Ok((Vec::new(), false));
        };
        // A worktree that doesn't exist yet starts from the repository's files.
        let path = match environment {
            Environment::LocalCheckout { .. } => repo,
            Environment::NewWorktree { path, .. } => path.unwrap_or(repo),
        };
        self.files_at(PathBuf::from(path), query).await
    }

    /// The files of a project's folder, as [`Self::list_files`] lists a session's: what the
    /// Files pane of a session not started yet shows.
    pub async fn list_project_files(
        &self,
        project: &ProjectId,
        query: Option<String>,
    ) -> Result<(Vec<String>, bool)> {
        let path = self.project_checkout(project)?;
        self.files_at(path, query).await
    }

    async fn files_at(&self, path: PathBuf, query: Option<String>) -> Result<(Vec<String>, bool)> {
        let git = self.git.clone();
        blocking(move || {
            git.open(&path)
                .and_then(|repo| repo.files(MENTION_FILES, query.as_deref()))
                .map_err(git_error)
        })
        .await
    }

    /// Branches and state of a repository, for the composer's pickers.
    pub async fn repo_info(&self, path: String) -> Result<RepoInfo> {
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(Path::new(&path)).map_err(git_error)?;
            let state = repo.state().map_err(git_error)?;
            let root = repo.root().to_string_lossy().into_owned();
            Ok(RepoInfo {
                name: Path::new(&root)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| root.clone()),
                path: root,
                current_branch: state.current_branch,
                dirty: !state.dirty_files.is_empty(),
                branches: state
                    .branches
                    .into_iter()
                    .map(|branch| crate::model::BranchInfo {
                        name: branch.name,
                        commit: branch.commit.0,
                        checked_out_at: branch
                            .checked_out_at
                            .map(|path| path.to_string_lossy().into_owned()),
                    })
                    .collect(),
            })
        })
        .await
    }

    /// What a worktree session's branch changed since it left its base, and how many commits
    /// it has that its base lacks (what "Merge into main" merges, even when they change nothing
    /// in sum). Absent for Chats, local-checkout sessions (their commits are on the picked
    /// branch) and a session whose branch or base does not exist (yet).
    pub async fn session_diff_stat(&self, id: &ConversationId) -> Result<Option<(DiffStat, u32)>> {
        let conversation = self.core.conversation(id)?;
        let Some(Setup::Session {
            repo,
            environment: Environment::NewWorktree { base, branch, .. },
            ..
        }) = conversation.setup
        else {
            return Ok(None);
        };
        let git = self.git.clone();
        let base_name = base.clone();
        blocking(move || {
            let repo = git.open(Path::new(&repo)).map_err(git_error)?;
            let (Some(base), Some(tip)) = (
                repo.branch_tip(&base).map_err(git_error)?,
                repo.branch_tip(&branch).map_err(git_error)?,
            ) else {
                return Ok(None);
            };
            let mut fork = repo.merge_base(&base, &tip).map_err(git_error)?;
            let mut from = base;
            // The branch merged in the base's upstream, newer than the local base (#233): what
            // it changed is counted from there, not from where it first left the base.
            if let Some(upstream) = repo.upstream_tip(&base_name).map_err(git_error)?
                && let Ok(newer) = repo.merge_base(&upstream, &tip)
                && newer != fork
                && repo.ancestor(&fork, &newer).map_err(git_error)?
            {
                fork = newer;
                from = upstream;
            }
            let stat = repo.diff_stat(&fork, &tip).map_err(git_error)?;
            let commits = repo.count_commits(&from, &tip).map_err(git_error)?;
            Ok(Some((landing::diff_stat_of(&stat), commits)))
        })
        .await
    }

    /// What each write task at work in the conversation (or reported, before its candidate
    /// commit exists) changed in its worktree so far. A task whose worktree can't be read (it
    /// is being set up or removed) is left out.
    pub async fn worker_diffs(&self, id: &ConversationId) -> Result<Vec<WorkerDiff>> {
        let board = self.core.board(id).await?;
        let parent_branch = self.changes_checkout(id).and_then(|(_, branch)| branch);
        let checkouts: Vec<(TaskId, PathBuf, String)> = board
            .tasks
            .values()
            .filter(|task| task.kind.writes())
            .filter_map(|task| {
                let workspace = task.workspace.as_ref()?;
                Some((
                    task.id.clone(),
                    PathBuf::from(workspace.worktree.as_ref()?),
                    parent_branch.clone()?,
                ))
            })
            .collect();
        if checkouts.is_empty() {
            return Ok(Vec::new());
        }
        let git = self.git.clone();
        blocking(move || {
            Ok(checkouts
                .into_iter()
                .filter_map(|(task_id, path, base)| {
                    let repo = git.open(&path).ok()?;
                    let stat = repo.worker_diff(&base).ok()?;
                    Some(WorkerDiff {
                        task_id,
                        stat: landing::diff_stat_of(&stat),
                    })
                })
                .collect())
        })
        .await
    }

    /// Keeps the user's rating of an answer or a plan, or with `None` takes it back. It stays
    /// on this machine.
    pub async fn rate(
        &self,
        id: &ConversationId,
        subject: String,
        rating: Option<Rating>,
    ) -> Result<()> {
        self.core.conversation(id)?;
        let event = match rating {
            Some(rating) => DomainEvent::MessageRated { subject, rating },
            None => DomainEvent::RatingCleared { subject },
        };
        self.core.record_conversation(id, vec![event]).await?;
        Ok(())
    }

    /// Creates a session or Chat. A session starts a workspace of its own, as its first tab. A
    /// local-checkout session asked to start on a new branch gets that branch created from
    /// `create_from` first.
    pub async fn create_conversation(
        &self,
        kind: ConversationKind,
        project_id: Option<ProjectId>,
        title: Option<String>,
        setup: Option<SetupRequest>,
    ) -> Result<Conversation> {
        let orchestrator = match &setup {
            Some(SetupRequest::Session { orchestrator, .. }) => Some(orchestrator),
            Some(SetupRequest::Chat { model }) => Some(model),
            None => None,
        };
        if let Some(choice) = orchestrator {
            self.check_choice(choice)?;
        }
        let _admitting = self.local_checkouts.lock().await;
        if let Some(SetupRequest::Session {
            repo,
            environment: EnvironmentRequest::LocalCheckout { .. },
            ..
        }) = &setup
            && let Some(holder) = self.checkout_holder(repo, None)
        {
            return Err(checkout_taken(&holder));
        }
        if let Some(SetupRequest::Session {
            repo,
            environment:
                EnvironmentRequest::LocalCheckout {
                    branch,
                    create_from: Some(from),
                },
            ..
        }) = &setup
        {
            let (git, at, made, from) =
                (self.git.clone(), repo.clone(), branch.clone(), from.clone());
            blocking(move || {
                let repo = git.open(Path::new(&at)).map_err(git_error)?;
                let start = repo.branch_commit(&from).map_err(git_error)?;
                repo.create_branch(&made, &start).map_err(git_error)
            })
            .await?;
            self.note_branch_made(Path::new(repo), branch).await;
        }
        let id = ConversationId::generate();
        let (prefix, _) = self.branch_prefix_of(project_id.as_ref());
        let setup = setup.map(|request| Setup::from_request(request, &id, &prefix));
        self.core
            .create_conversation(id, kind, project_id, title, setup, Origin::default())
            .await
    }

    /// The other session that works in `repo`'s own folder, if one does: an open one, or one
    /// archived or deleted whose cleanup hasn't stopped it yet. Two sessions there would edit,
    /// commit and switch branches over each other.
    fn checkout_holder(&self, repo: &str, except: Option<&ConversationId>) -> Option<Conversation> {
        self.core.catalog().conversations.into_iter().find(|other| {
            Some(&other.id) != except
                && (other.lifecycle != Lifecycle::Archived
                    || other.cleanup_pending
                    || other.deleting)
                && matches!(&other.setup, Some(Setup::Session {
                        repo: at,
                        environment: Environment::LocalCheckout { .. },
                        ..
                    }) if at == repo)
        })
    }

    fn arc(&self) -> Arc<Self> {
        self.me
            .upgrade()
            .expect("the session manager is alive while in use")
    }

    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        let task = self.background.track_future(task);
        (self.spawner)(Box::pin(task));
    }

    fn admit(&self) -> Result<()> {
        if self.admitting.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(Error::Invalid("Brigadier is shutting down".into()))
        }
    }

    fn convs_lock(&self) -> MutexGuard<'_, HashMap<ConversationId, Arc<ConvLive>>> {
        self.convs.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn tasks_lock(&self) -> MutexGuard<'_, HashMap<TaskId, Arc<TaskLive>>> {
        self.tasks.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn stopping_lock(&self) -> MutexGuard<'_, HashSet<TaskId>> {
        self.stopping.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn user_stops_lock(&self) -> MutexGuard<'_, HashSet<TaskId>> {
        self.user_stops.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Records that the user is stopping `task`, before the stop runs.
    pub(crate) fn mark_user_stop(&self, task: &TaskId) {
        self.user_stops_lock().insert(task.clone());
    }

    /// Whether the user stopped `task`: being stopped by them now, or filed "Stopped by the
    /// user" on `board` (which outlives a restart).
    pub(crate) fn user_stopped(&self, board: &crate::board::Board, task: &TaskId) -> bool {
        self.user_stops_lock().contains(task)
            || board.orchestrator_steps.iter().any(|step| {
                matches!(&step.kind, crate::work::OrchestratorStepKind::Stopped { task_id, reason }
                    if task_id == task && reason == workers::USER_STOP_REASON)
            })
    }

    /// Whether `task` is being stopped right now.
    pub(crate) fn is_stopping(&self, task: &TaskId) -> bool {
        self.stopping_lock().contains(task)
    }

    /// The live state of a conversation, created on first use.
    fn conv(&self, id: &ConversationId) -> Result<Arc<ConvLive>> {
        let conversation = self.core.conversation(id)?;
        let mut convs = self.convs_lock();
        Ok(convs
            .entry(id.clone())
            .or_insert_with(|| {
                Arc::new(ConvLive::new(
                    conversation.id.clone(),
                    conversation.kind,
                    conversation.tab_chat.is_some(),
                ))
            })
            .clone())
    }

    /// A Brigadier-owned folder of conversation or job `id`: a session's own areas are parts of
    /// its folder ([`session_dir`]): the thread's files (`orch`) in `dev/`, a Chat's working
    /// folder in `work/`, previews' logs in `logs/`, the workers' home in `workers/home`.
    /// Anything else is `<data>/<area>/<id>`.
    fn owned_dir(&self, area: &str, id: &str) -> PathBuf {
        let session = || self.session_dir(&ConversationId(id.to_owned()));
        match area {
            "orch" => session().dev(),
            "chat" => session().work(),
            preview::LOG_AREA => session().logs(),
            "worker-home" => session().workers_home(),
            _ => self.data_dir.join(area).join(id),
        }
    }

    /// A CLI session's event as its handlers get it: in development builds, with what an
    /// armed fault ([`fault`]) adds around it.
    fn session_events(
        &self,
        source: EventSource<'_>,
        cli: &Arc<conversation::Cli>,
        event: ProviderEvent,
    ) -> Vec<ProviderEvent> {
        #[cfg(debug_assertions)]
        {
            let key = match source {
                EventSource::Task(id) => fault::FaultKey::Task(id.clone()),
                EventSource::Conversation(id) => fault::FaultKey::Conversation(id.clone()),
            };
            self.fault_events(key, cli, event)
        }
        #[cfg(not(debug_assertions))]
        {
            let _ = (source, cli);
            vec![event]
        }
    }

    /// Whether a new conversation may start on `choice`, or one switch to it: its agent is on
    /// and the model it names is available ([`crate::routing::availability::check_choice`]).
    fn check_choice(&self, choice: &ModelChoice) -> Result<()> {
        let catalog = self
            .runtime
            .overview(choice.provider)
            .and_then(|overview| overview.models)
            .map(|catalog| catalog.models)
            .unwrap_or_default();
        crate::routing::availability::check_choice(&self.core.settings(), choice, &catalog)
            .map_err(Error::Invalid)
    }

    /// Changes a conversation's setup ([`Core::set_setup`]). Switching to a model the user
    /// made unavailable isn't allowed; staying on one is.
    pub async fn set_setup(&self, id: ConversationId, setup: Setup) -> Result<Conversation> {
        let current = self.core.conversation(&id)?.setup;
        let same_model = current.as_ref().is_some_and(|current| {
            let (now, next) = (current.choice(), setup.choice());
            now.provider == next.provider && now.model == next.model
        });
        if !same_model {
            self.check_choice(setup.choice())?;
        }
        // A session given its environment only now is let into the checkout as a new one is.
        if current.is_none()
            && let Setup::Session {
                repo,
                environment: Environment::LocalCheckout { .. },
                ..
            } = &setup
        {
            let _admitting = self.local_checkouts.lock().await;
            if let Some(holder) = self.checkout_holder(repo, Some(&id)) {
                return Err(checkout_taken(&holder));
            }
            return self.core.set_setup(id, setup).await;
        }
        self.core.set_setup(id, setup).await
    }

    /// Switched on, logged in and not refusing work (see [`Self::provider_ready`]).
    fn provider_usable(&self, kind: ProviderKind) -> bool {
        crate::routing::availability::provider_on(&self.core.settings(), kind)
            && self.provider_ready(kind)
    }

    /// Logged in and not refusing work: no limit, or one whose reset has passed (the quota
    /// monitor's view). Whether the user switched it off is not asked: a conversation already
    /// running on it goes on.
    fn provider_ready(&self, kind: ProviderKind) -> bool {
        self.runtime.provider_signed_in(kind)
            && self
                .runtime
                .monitor()
                .current(kind, crate::now_ms())
                .is_none_or(|quota| quota.limit.is_none())
    }
}

/// Why a new session can't work in the checkout `holder` works in.
fn checkout_taken(holder: &Conversation) -> Error {
    Error::Invalid(format!(
        "“{}” already works in this checkout. Archive it, or start this session in a new worktree.",
        holder.title
    ))
}

/// Whose CLI session an event comes from (development builds route faults by it).
#[derive(Clone, Copy)]
#[cfg_attr(not(debug_assertions), allow(dead_code))]
enum EventSource<'a> {
    Task(&'a TaskId),
    Conversation(&'a ConversationId),
}

impl ToolHost for SessionManager {
    fn role(&self, grant: &str) -> Option<Role> {
        self.grants.resolve(grant)
    }

    fn call(&self, grant: &str, call: ToolCall) -> BoxFuture<'_, ToolReply> {
        let role = self.grants.resolve(grant);
        let manager = self.arc();
        Box::pin(async move {
            let Some(role) = role else {
                return ToolReply::error("This grant is not valid (the session ended).");
            };
            match (role, call) {
                (
                    Role::Orchestrator {
                        conversation_id, ..
                    },
                    ToolCall::Orchestrator(call),
                ) => {
                    // On the heap, as the worker's and a Chat's arms: a debug build gives each
                    // unboxed await of these big futures its own room in this frame, which a
                    // landing flow runs through (#354).
                    crate::heap(|| manager.orchestrator_call(conversation_id, call)).await
                }
                (
                    Role::Worker {
                        conversation_id,
                        task_id,
                        ..
                    },
                    ToolCall::Worker(call),
                ) => crate::heap(|| manager.worker_call(conversation_id, task_id, call)).await,
                (
                    Role::Orchestrator {
                        conversation_id, ..
                    },
                    ToolCall::Browser(call),
                ) => {
                    // On the heap: its guard's wait makes a future big enough that every other
                    // arm's caller (a landing flow) ran out of a runtime thread's stack.
                    crate::heap(|| manager.browser_tool(conversation_id, None, call)).await
                }
                (
                    Role::Worker {
                        conversation_id,
                        task_id,
                        ..
                    },
                    ToolCall::Browser(call),
                ) => {
                    crate::heap(|| manager.browser_tool(conversation_id, Some(task_id), call)).await
                }
                (Role::BrainJob { project_id, job_id }, ToolCall::Job(call)) => {
                    manager.job_call(project_id, job_id, call).await
                }
                (Role::Chat { conversation_id }, ToolCall::Chat(call)) => {
                    manager.chat_call(conversation_id, call).await
                }
                (Role::Chat { conversation_id }, ToolCall::Orchestrator(call))
                    if call.in_chat() =>
                {
                    crate::heap(|| manager.orchestrator_call(conversation_id, call)).await
                }
                (Role::Chat { conversation_id }, ToolCall::Browser(call)) => {
                    crate::heap(|| manager.browser_tool(conversation_id, None, call)).await
                }
                // The app tools (Q90) are the thread's and a Chat's: workers work in their
                // worktrees, and the user watches the thread.
                (
                    Role::Orchestrator {
                        conversation_id, ..
                    }
                    | Role::Chat { conversation_id },
                    ToolCall::App(call),
                ) => crate::heap(|| manager.app_tool(conversation_id, call)).await,
                (
                    Role::TabChat {
                        conversation_id, ..
                    },
                    ToolCall::TabChat(call),
                ) => manager.tab_chat_call(conversation_id, call).await,
                (Role::NoteFork { .. }, _) => ToolReply::error(rebirth::NOTE_FORK_REFUSAL),
                _ => ToolReply::error("This tool is not available to this session."),
            }
        })
    }

    fn bad_call(&self, grant: &str, tool: &str) {
        let Some(store) = self.runtime.routing_store().cloned() else {
            return;
        };
        let (conversation, task) = match self.grants.resolve(grant) {
            Some(Role::Worker {
                conversation_id,
                task_id,
                ..
            }) => (Some(conversation_id.0), Some(task_id.0)),
            Some(
                Role::Orchestrator {
                    conversation_id, ..
                }
                | Role::Chat { conversation_id }
                | Role::TabChat {
                    conversation_id, ..
                },
            ) => (Some(conversation_id.0), None),
            _ => (None, None),
        };
        let tool = tool.to_owned();
        tokio::spawn(async move {
            if let Err(err) = store
                .add_bad_tool_call(crate::now_ms(), conversation, task, tool)
                .await
            {
                tracing::warn!(error = %err, "could not count a bad tool call");
            }
        });
    }
}

/// Files of a session's checkout listed for @-mentions; a bigger checkout lists the first ones.
const MENTION_FILES: usize = 20_000;

/// Runs blocking work (git, file copies) off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|err| Error::Invalid(format!("background work failed: {err}")))?
}

fn git_error(err: brigadier_git::Error) -> Error {
    match err {
        brigadier_git::Error::NotARepository(path) => {
            Error::Invalid(format!("{} is not a git repository", path.display()))
        }
        other => Error::Invalid(other.to_string()),
    }
}
