//! The session manager: live sessions, Chats and their workers.
//!
//! It turns the conversation log into running CLI sessions and back:
//!
//! - one thread per Brigadier session ([`conversation`], [`thread`]): a CLI session that
//!   reads, runs and makes tiny edits in the session's workspace and calls the Brigadier MCP
//!   tools ([`tools`]);
//! - workers, one CLI session per task, in their own worktrees ([`workers`]), whose accepted
//!   work lands as one reviewed commit ([`landing`]);
//! - one CLI session per Chat, with no orchestrator and no workers;
//! - the cards the user answers ([`cards`]), the grants the sessions hold, and the lifecycle
//!   (hibernate, archive, restore, delete) with the cleanup of everything they create
//!   ([`lifecycle`]).
//!
//! Every CLI session is disposable: the conversation stream is the truth, and a session that
//! is gone (hibernated with its files cleaned up, archived, crashed) is started again from it.

mod add_project;
mod brain_jobs;
mod brains;
mod branches;
mod cards;
mod closing;
mod cold;
mod context_pack;
mod conversation;
pub(crate) mod decisions;
pub mod disk;
mod engine;
mod fallback;
#[cfg(debug_assertions)]
pub mod fault;
mod files;
#[cfg(all(test, unix))]
mod flow;
mod fork;
mod git_actions;
mod instructions;
mod landing;
mod lifecycle;
mod machine;
mod outcomes;
mod outputs;
pub mod overnight;
mod past_projects;
mod phases;
mod preview;
mod project_removal;
mod prompts;
mod pull_request;
mod quiet;
pub(crate) mod reads;
mod rebirth;
mod requests;
mod research;
mod review;
mod review_runs;
mod routing;
mod run;
mod secrets;
mod side_chat;
mod thread;
mod thread_metrics;
mod tool_output;
mod tools;
mod undo;
mod uninstall;
mod usage;
mod usage_view;
pub mod warm;
mod watchdog;
mod worker_handoff;
mod workers;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use brigadier_git::Git;
use brigadier_providers::{BoxFuture, ProviderEvent, ProviderKind};
use tokio_util::task::TaskTracker;

use crate::model::{
    Conversation, ConversationId, ConversationKind, DomainEvent, Environment, EnvironmentRequest,
    ModelChoice, ProjectId, Rating, RepoInfo, Setup, SetupRequest,
};
use crate::runtime::{Runtime, Spawner};
use crate::sessions::Origin;
use crate::tools::{Grants, Role, ToolCall, ToolHost, ToolReply};
use crate::work::{DiffStat, TaskId, TaskState, WorkerDiff};
use crate::{Core, Error, Result};

pub use brains::{BrainCounters, IndexRunStats};
pub use closing::Turn;
pub use conversation::SendOutcome;
pub use tool_output::{HOOK_GRANT_ENV, HookOutput, OUTPUT_MAX_BYTES};
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
    convs: Mutex<HashMap<ConversationId, Arc<ConvLive>>>,
    tasks: Mutex<HashMap<TaskId, Arc<TaskLive>>>,
    /// Held while a new-worktree session's own worktree is created, so parallel first tasks
    /// create it once.
    session_worktrees: tokio::sync::Mutex<()>,
    /// Held while a plan is proposed (checked against the plans as they are, then recorded
    /// and the open ones replaced) and while a review result decides a plan, so the two
    /// can't interleave.
    plans: tokio::sync::Mutex<()>,
    /// Held while a one-shot review is looked for and recorded, so a range is reviewed once.
    reviews: tokio::sync::Mutex<()>,
    /// Held while the thread's new commits are looked for and their tip recorded
    /// ([`thread`]), so a range is taken once.
    thread_scans: tokio::sync::Mutex<()>,
    /// Commands a Codex thread's `run_unsandboxed` may run, each once: what the user (or their
    /// "allow similar") approved when Codex asked ([`run`]).
    run_passes: run::RunPasses,
    /// The previews running now ([`preview`]).
    previews: preview::Previews,
    /// The one-shot reviews running now, by id: their conversation, and what ends one when
    /// its conversation closes.
    running_reviews: Mutex<HashMap<String, (ConversationId, tokio_util::sync::CancellationToken)>>,
    /// Held while a task is read, changed and recorded (`update_task`), so two writers can't
    /// each write back a copy that lacks the other's change.
    task_writes: tokio::sync::Mutex<()>,
    /// Held while "Waiting on you" items are read, added and resolved, so one is never added
    /// twice.
    waiting: tokio::sync::Mutex<()>,
    /// Tasks being stopped: a landing that fails meanwhile (its worktree going away under
    /// it) is no news for the orchestrator.
    stopping: Mutex<HashSet<TaskId>>,
    waiters: Waiters,
    admitting: AtomicBool,
    background: TaskTracker,
    brains: brains::Brains,
    /// Development builds: usage limits armed to exercise fallback.
    #[cfg(debug_assertions)]
    faults: fault::Faults,
    research: research::Research,
    overnight: overnight::Runs,
    /// Holds new work while the machine struggles, and runs heavy commands one at a time.
    machine: Arc<crate::machine::MachineWatch>,
    /// Conversations being archived or deleted: their fences and cleanups.
    closing: closing::Closing,
    /// Background model turns, for maintenance's idle check.
    activity: Arc<quiet::Activity>,
}

impl SessionManager {
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
            data_dir.join("stopped-processes.json"),
        ));
        let manager = Arc::new_cyclic(|me| Self {
            me: me.clone(),
            core,
            runtime,
            spawner,
            config,
            git,
            data_dir,
            grants: Grants::default(),
            convs: Mutex::new(HashMap::new()),
            tasks: Mutex::new(HashMap::new()),
            session_worktrees: tokio::sync::Mutex::new(()),
            plans: tokio::sync::Mutex::new(()),
            reviews: tokio::sync::Mutex::new(()),
            thread_scans: tokio::sync::Mutex::new(()),
            run_passes: run::RunPasses::default(),
            previews: preview::Previews::default(),
            running_reviews: Mutex::default(),
            task_writes: tokio::sync::Mutex::new(()),
            waiting: tokio::sync::Mutex::new(()),
            stopping: Mutex::new(HashSet::new()),
            waiters: Waiters::default(),
            admitting: AtomicBool::new(true),
            background: TaskTracker::new(),
            brains,
            #[cfg(debug_assertions)]
            faults: fault::Faults::default(),
            research: research::Research::default(),
            overnight: overnight::Runs::default(),
            machine,
            closing: closing::Closing::default(),
            activity: Arc::default(),
        });
        manager.install_worktree_remover();
        manager.start_machine_watch().await;
        // Cleanups a quit cut off finish before anything of those conversations resumes.
        manager.finish_cut_off_cleanups().await;
        // The thread engine's first start deletes the earlier engine's conversations, before
        // anything of theirs could resume.
        manager.switch_engine().await?;
        manager.recover_active_runs().await;
        manager.recover().await;
        manager.resume_runs().await;
        manager.start_overnight_clock();
        manager.open_brains().await;
        manager.start_hibernation_timer();
        manager.start_watchdog();
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
        // What Brigadier stopped goes on before the CLIs above it close.
        self.quit_machine_watch().await;
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
        let git = self.git.clone();
        blocking(move || {
            git.open(Path::new(&path))
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

    /// What a worktree session's branch changed since it left its base. Absent for Chats,
    /// local-checkout sessions (their commits are on the picked branch) and a session whose
    /// branch or base does not exist (yet).
    pub async fn session_diff_stat(&self, id: &ConversationId) -> Result<Option<DiffStat>> {
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
        blocking(move || {
            let repo = git.open(Path::new(&repo)).map_err(git_error)?;
            let (Some(base), Some(tip)) = (
                repo.branch_tip(&base).map_err(git_error)?,
                repo.branch_tip(&branch).map_err(git_error)?,
            ) else {
                return Ok(None);
            };
            let fork = repo.merge_base(&base, &tip).map_err(git_error)?;
            let stat = repo.diff_stat(&fork, &tip).map_err(git_error)?;
            Ok(Some(landing::diff_stat_of(&stat)))
        })
        .await
    }

    /// What each write task at work in the conversation (or reported, before its candidate
    /// commit exists) changed in its worktree so far. A task whose worktree can't be read (it
    /// is being set up or removed) is left out.
    pub async fn worker_diffs(&self, id: &ConversationId) -> Result<Vec<WorkerDiff>> {
        let board = self.core.board(id).await?;
        let checkouts: Vec<(TaskId, PathBuf, String)> = board
            .tasks
            .values()
            .filter(|task| {
                task.kind.writes()
                    && matches!(
                        task.state,
                        TaskState::Running
                            | TaskState::Blocked
                            | TaskState::Paused
                            | TaskState::Reported
                    )
            })
            .filter_map(|task| {
                let workspace = task.workspace.as_ref()?;
                Some((
                    task.id.clone(),
                    PathBuf::from(workspace.worktree.as_ref()?),
                    workspace.base.clone()?,
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
                    let files = repo.files_tree().ok()?;
                    let stat = repo.diff_stat(&brigadier_git::Oid(base), &files).ok()?;
                    Some(WorkerDiff {
                        task_id,
                        stat: landing::diff_stat_of(&stat),
                    })
                })
                .collect())
        })
        .await
    }

    /// Keeps the user's rating of an answer. It stays on this machine.
    pub async fn rate(&self, id: &ConversationId, subject: String, rating: Rating) -> Result<()> {
        self.core.conversation(id)?;
        self.core
            .record_conversation(id, vec![DomainEvent::MessageRated { subject, rating }])
            .await?;
        Ok(())
    }

    /// Creates a session or Chat. A local-checkout session asked to start on a new branch gets
    /// that branch created from `create_from` first.
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
            let (git, repo, branch, from) =
                (self.git.clone(), repo.clone(), branch.clone(), from.clone());
            blocking(move || {
                let repo = git.open(Path::new(&repo)).map_err(git_error)?;
                let start = repo.branch_commit(&from).map_err(git_error)?;
                repo.create_branch(&branch, &start).map_err(git_error)
            })
            .await?;
        }
        let id = ConversationId::generate();
        let setup = setup.map(|request| Setup::from_request(request, &id));
        self.core
            .create_conversation(id, kind, project_id, title, setup, Origin::default())
            .await
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
            .or_insert_with(|| Arc::new(ConvLive::new(conversation.id.clone(), conversation.kind)))
            .clone())
    }

    /// `<data>/<area>/<id>`: a Brigadier-owned folder.
    fn owned_dir(&self, area: &str, id: &str) -> PathBuf {
        self.data_dir.join(area).join(id)
    }

    /// The IPC socket CLIs' sandboxes must be able to reach (the MCP bridge).
    fn socket_path(&self) -> Option<PathBuf> {
        match &self.runtime.platform().paths().ipc_endpoint {
            brigadier_sandbox::IpcEndpoint::UnixSocket(path) => Some(path.clone()),
            #[allow(unreachable_patterns)]
            _ => None,
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
        self.runtime.overview(kind).is_some_and(|overview| {
            overview
                .status
                .as_ref()
                .is_some_and(|status| status.logged_in)
        }) && self
            .runtime
            .monitor()
            .current(kind, crate::now_ms())
            .is_none_or(|quota| quota.limit.is_none())
    }
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
                ) => manager.orchestrator_call(conversation_id, call).await,
                (
                    Role::Worker {
                        conversation_id,
                        task_id,
                        ..
                    },
                    ToolCall::Worker(call),
                ) => manager.worker_call(conversation_id, task_id, call).await,
                (Role::BrainJob { project_id, job_id }, ToolCall::Job(call)) => {
                    manager.job_call(project_id, job_id, call).await
                }
                (Role::Chat { conversation_id }, ToolCall::Chat(call)) => {
                    manager.chat_call(conversation_id, call).await
                }
                _ => ToolReply::error("This tool is not available to this session."),
            }
        })
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
