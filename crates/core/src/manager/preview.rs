//! Previews (THREAD-PLAN.md Q6): what the thread starts so the user can see its work running (a
//! dev server, the app), with `start_preview`, `stop_preview` and `preview_log`.
//!
//! A preview is the daemon's own process, not the CLI's:
//! - It leads its own process group and is recorded under its own cleanup-ledger owner,
//!   `preview:<conversation>`, never under the thread's (`orch:<id>`). Ending the thread's CLI
//!   (provider teardown on hibernation, rebirth or a vendor fallback) and ending the thread
//!   owner's processes (`end_processes`, whose `end_in_dir` sweeps only the thread's scratch
//!   folder) never reach it: the workspace it runs in is never recorded under the thread's
//!   owner (see [`super::thread`]).
//! - It runs in the thread's effective workspace (a folder inside it), and belongs to that
//!   workspace: when the workspace changes (an overnight run starts or ends) it stops.
//! - It stops on `stop_preview`, the user's Stop, the merge of the session's work (a thread's
//!   preview started to show it, #289; not one the user asked for, #335), archive and delete (before the `session:<id>` owner's worktree goes) and Brigadier's quit; the launch
//!   sweep ends any a crash left running, and its record reads as stopped.
//! - Stopping it is SIGTERM to its process group, then, after [`STOP_GRACE`], SIGKILL to it
//!   and everything it started. The grace is for all of them, not only the leader: one that
//!   quits at once doesn't cut short the others' handling of SIGTERM.
//! - Its stdout and stderr go to one log file in `<data>/previews/<conversation>`. Reading it
//!   (`preview_log`) stores a snapshot in the blob store under an `out-<id>` alias, as a long
//!   command output is (`super::tool_output`), and the final log is stored when it ends.
//!
//! **Access.** The daemon starts it as a plain process in its own process group, as the
//! user's terminal would: Brigadier adds no sandbox (G5).
//!
//! **Brigadier itself.** A preview never touches the installed app's data: the daemon's own
//! `BRIGADIER_DATA_DIR` is never passed on, one the thread sets may not name the installed
//! app's data folder or this daemon's, and in Brigadier's own repository the thread must set a
//! scratch one (the app then runs under a dev identity, never `ai.brigadier.app`).

use std::collections::{BTreeMap, HashMap};
use std::io::{Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::Artifact;
use tokio_util::sync::CancellationToken;

use super::{OUTPUT_MAX_BYTES, SessionManager, blocking};
use crate::model::{ConversationId, DomainEvent, Lifecycle, TaskId};
use crate::tools::StartPreview;
use crate::work::{OutputSource, Preview, PreviewKind, PreviewState};
use crate::{Error, Result, now_ms};

/// How long a stopped preview has to exit after SIGTERM before it is killed.
/// Why a preview the user stopped ended: their Stop on its chip, or on the conversation.
pub(crate) const USER_STOP: &str = "stopped by the user";
/// Why a thread's preview ended at a merge: the work it showed is merged (#289).
pub(crate) const MERGED_STOP: &str = "the work it showed is merged";
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How often a stop looks whether what the leader left has quit.
const STOP_POLL: Duration = Duration::from_millis(20);
/// How long `start_preview` watches a new preview for an early exit (or its readiness) before
/// it answers.
const START_WATCH: Duration = Duration::from_millis(1500);
/// How often a starting preview is checked for readiness.
const READY_POLL: Duration = Duration::from_secs(1);
/// How much of a starting preview's log end is read for a build under way.
const BUILD_SCAN: u64 = 16 * 1024;
/// The longest `preview_log` waits for a starting preview.
const WAIT_MAX_SECS: u32 = 300;
/// Lines `preview_log` shows by default, and at most.
const TAIL_DEFAULT: u32 = 40;
const TAIL_MAX: u32 = 400;
/// Lines of output `start_preview` shows.
const START_TAIL: u32 = 20;
/// Most bytes of a log's end a reply shows.
const TAIL_BYTES_MAX: usize = 8 * 1024;
/// The data-folder area of the previews' logs.
pub(super) const LOG_AREA: &str = "previews";

/// The cleanup-ledger owner of a conversation's previews.
pub(crate) fn preview_owner(id: &ConversationId) -> String {
    format!("preview:{id}")
}

/// Who starts a preview, and where: the thread in the session's workspace, or a worker in its
/// own worktree (or scratch folder).
struct Starter {
    /// The folder it belongs to; `workdir` must be inside it (or inside `scratch` for a
    /// worker).
    workspace: PathBuf,
    /// The starter's scratch folder, for its session environment.
    scratch: PathBuf,
    /// The worker's task; none for the thread.
    task: Option<TaskId>,
}

/// A running preview.
pub(crate) struct LivePreview {
    conversation: ConversationId,
    id: String,
    /// The worker that started it; none for the thread.
    task: Option<TaskId>,
    /// The user asked for it (`purpose: "user"`): a merge leaves it running (#335).
    for_user: bool,
    pid: u32,
    kind: PreviewKind,
    workspace: PathBuf,
    /// A PDF preview's file: it is ready once that is there.
    pdf: Option<PathBuf>,
    log: PathBuf,
    /// Cancelled once it is ready to look at (it moved from Starting to Running).
    ready: CancellationToken,
    /// Why it is being stopped (the first reason given wins).
    reason: Mutex<Option<String>>,
    /// Asks its watcher to stop it.
    stop: CancellationToken,
    /// Cancelled once its end is recorded.
    ended: CancellationToken,
    /// The log's size and alias at its last snapshot, so an unchanged log isn't stored again.
    snapshot: Mutex<Option<(u64, String)>>,
    /// The descendants suspended with it, including ones outside its group.
    paused_members: Mutex<Vec<crate::machine::Proc>>,
    /// Held while its record is read and written back, so a log snapshot can't undo its end.
    updating: tokio::sync::Mutex<()>,
    /// Whether `start_preview` answered (that it is starting or running), and whether its end
    /// began: whichever
    /// comes first decides who tells the thread of an end on its own (the start's answer,
    /// or a message of its own).
    start_told: Mutex<StartTold>,
}

#[derive(Default)]
struct StartTold {
    answered: bool,
    ending: bool,
}

impl LivePreview {
    fn ask_to_stop(&self, reason: &str) {
        self.reason
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_or_insert_with(|| reason.to_owned());
        self.stop.cancel();
    }

    fn reason(&self) -> String {
        self.reason
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_else(|| "stopped".into())
    }
}

/// The previews running now.
#[derive(Default)]
pub(crate) struct Previews {
    live: Mutex<HashMap<(ConversationId, String), Arc<LivePreview>>>,
    /// Held while a preview is numbered, started and recorded; the recording task holds it
    /// to the end, so a cancelled start can't free its number early.
    pub(crate) starting: Arc<tokio::sync::Mutex<()>>,
    /// How many times each conversation's previews were all stopped: a start under way when
    /// that happened is refused ([`SessionManager::stop_previews`]).
    stops: Mutex<HashMap<ConversationId, u64>>,
    /// Tasks whose folder is going: their worker's starts are refused
    /// ([`SessionManager::stop_task_previews`]) until it is gone with their worker
    /// ([`SessionManager::forget_task_previews`]).
    ended_tasks: Mutex<std::collections::HashSet<TaskId>>,
}

impl Previews {
    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<(ConversationId, String), Arc<LivePreview>>> {
        self.live.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn of(&self, id: &ConversationId) -> Vec<Arc<LivePreview>> {
        self.lock()
            .values()
            .filter(|live| &live.conversation == id)
            .cloned()
            .collect()
    }

    pub(crate) fn get(&self, id: &ConversationId, preview: &str) -> Option<Arc<LivePreview>> {
        self.lock().get(&(id.clone(), preview.to_owned())).cloned()
    }

    fn all(&self) -> Vec<Arc<LivePreview>> {
        self.lock().values().cloned().collect()
    }

    pub(super) fn stops_of(&self, id: &ConversationId) -> u64 {
        let stops = self.stops.lock().unwrap_or_else(|p| p.into_inner());
        stops.get(id).copied().unwrap_or_default()
    }

    pub(super) fn task_ended(&self, task: &TaskId) -> bool {
        let ended = self.ended_tasks.lock().unwrap_or_else(|p| p.into_inner());
        ended.contains(task)
    }

    fn count_stop(&self, id: &ConversationId) {
        let mut stops = self.stops.lock().unwrap_or_else(|p| p.into_inner());
        *stops.entry(id.clone()).or_default() += 1;
    }
}

impl SessionManager {
    /// `start_preview`: starts `args.command` in the thread's workspace and answers once it is
    /// running (or has already ended), with its first output.
    pub(crate) async fn start_preview(
        &self,
        id: &ConversationId,
        args: StartPreview,
    ) -> Result<String> {
        self.check_preview_start(id)?;
        let stops = self.previews.stops_of(id);
        let cli = self
            .conv(id)?
            .live_cli()
            .await
            .ok_or_else(|| Error::Invalid("the session's thread has ended".into()))?;
        let launch = cli
            .launch
            .as_ref()
            .ok_or_else(|| Error::Invalid("only a session's thread starts previews".into()))?;
        let workspace = launch
            .workspace
            .as_ref()
            .map(|workspace| workspace.path.clone())
            .ok_or_else(|| Error::Invalid("this session has no workspace yet".into()))?;
        if self.recorded_workspace(id).map(|now| now.path).as_ref() != Some(&workspace) {
            return Err(Error::Invalid(
                "the session's workspace is changing: start the preview again in your next turn"
                    .into(),
            ));
        }
        let starter = Starter {
            workspace,
            scratch: self.owned_dir("orch", &id.0),
            task: None,
        };
        self.start_preview_as(id, args, starter, stops, None).await
    }

    /// A worker's `start_preview`: starts `args.command` in its worktree (or scratch folder),
    /// with its access. It outlives the worker's report, and stops when its task's folder goes.
    pub(crate) async fn start_worker_preview(
        &self,
        id: &ConversationId,
        task_id: &TaskId,
        args: StartPreview,
    ) -> Result<String> {
        self.check_preview_start(id)?;
        let stops = self.previews.stops_of(id);
        let starter = self.worker_starter(id, task_id).await?;
        self.start_preview_as(id, args, starter, stops, None).await
    }

    /// Who starts a preview for the worker of `task_id`: in its worktree (or scratch folder),
    /// while it runs.
    async fn worker_starter(&self, id: &ConversationId, task_id: &TaskId) -> Result<Starter> {
        match self.existing_task_live(task_id) {
            Some(live) => live.tool_access().await,
            None => None,
        }
        .ok_or_else(|| Error::Invalid("the task's worker isn't running".into()))?;
        let task = self.task_by_id(id, task_id).await?;
        let workspace = task
            .workspace
            .ok_or_else(|| Error::Invalid("the task has no folder yet".into()))?;
        let scratch = PathBuf::from(&workspace.scratch);
        let starter = Starter {
            workspace: workspace
                .worktree
                .map_or_else(|| scratch.clone(), PathBuf::from),
            scratch,
            task: Some(task_id.clone()),
        };
        Ok(starter)
    }

    /// The user's Start again on an ended preview: its own command again, from its record, in
    /// the same folder with the same variables, under the same id, on its old port when that is
    /// free (as `PORT`, for servers that take it). A thread's runs with the session's access now,
    /// asleep or not; a worker's only while that worker runs. It runs for the user (a merge
    /// leaves it running), and the thread hears of it.
    pub async fn restart_preview(&self, id: ConversationId, preview_id: String) -> Result<()> {
        self.check_preview_start(&id)?;
        let stops = self.previews.stops_of(&id);
        let board = self.core.board(&id).await?;
        let preview =
            board.previews.get(&preview_id).cloned().ok_or_else(|| {
                Error::Invalid(format!("there is no {preview_id} in this session"))
            })?;
        if preview.state.is_live() || self.previews.get(&id, &preview_id).is_some() {
            return Err(Error::Invalid(format!("{preview_id} is running already")));
        }
        let starter = match &preview.task_id {
            Some(task) => self.worker_starter(&id, task).await.map_err(|_| {
                Error::Invalid(
                    "the worker that started it has ended: ask the thread to start it again".into(),
                )
            })?,
            None => {
                let scratch = self.owned_dir("orch", &id.0);
                let workspace = self
                    .recorded_workspace(&id)
                    .map(|workspace| workspace.path)
                    .ok_or_else(|| Error::Invalid("this session has no workspace now".into()))?;
                Starter {
                    workspace,
                    scratch,
                    task: None,
                }
            }
        };
        if starter.workspace != Path::new(&preview.workspace)
            && starter.scratch != Path::new(&preview.workspace)
        {
            return Err(Error::Invalid(
                "the session's workspace changed since it ran: ask the thread to start it again"
                    .into(),
            ));
        }
        let mut env = preview.env.clone();
        if let Some(port) = preview.url.as_deref().and_then(url_port)
            && !env.contains_key("PORT")
            && port_free(port).await
        {
            env.insert("PORT".into(), port.to_string());
        }
        let args = StartPreview {
            command: preview.command.clone(),
            name: Some(preview.name.clone()),
            env: Some(env),
            workdir: Some(preview.workdir.clone()),
            kind: Some(preview.kind),
            target: preview.target.clone(),
            purpose: Some(crate::tools::PreviewPurpose::User),
        };
        self.start_preview_as(&id, args, starter, stops, Some(preview_id.clone()))
            .await?;
        if let Ok(conv) = self.conv(&id) {
            conv.note(format!(
                "[preview] The user started {preview_id} \"{}\" again (its own command, in its \
                 folder); check preview_log before you tell them how it runs.",
                preview.name
            ))
            .await;
        }
        Ok(())
    }

    /// Whether a preview may start in conversation `id` now.
    fn check_preview_start(&self, id: &ConversationId) -> Result<()> {
        if cfg!(windows) {
            return Err(Error::Invalid(
                "previews run on macOS and Linux only".into(),
            ));
        }
        self.admit()?;
        let conversation = self.core.conversation(id)?;
        if conversation.lifecycle == Lifecycle::Archived || conversation.deleting {
            return Err(Error::Invalid("this session is closed".into()));
        }
        Ok(())
    }

    /// Starts `args.command` for `starter`; `stops` is how many times the conversation's
    /// previews were all stopped when the call came (a start under way then is refused).
    async fn start_preview_as(
        &self,
        id: &ConversationId,
        args: StartPreview,
        starter: Starter,
        stops: u64,
        again: Option<String>,
    ) -> Result<String> {
        let Starter {
            workspace,
            scratch,
            task,
        } = starter;
        let command = args.command.trim().to_owned();
        if command.is_empty() {
            return Err(Error::Invalid("`command` is empty".into()));
        }
        // A worker may also run it in its scratch folder.
        let workdir =
            preview_workdir(args.workdir.as_deref(), &workspace).or_else(|err| {
                match args.workdir.as_deref() {
                    Some(dir) if task.is_some() && scratch != workspace => {
                        preview_workdir(Some(dir), &scratch).map_err(|_| err)
                    }
                    _ => Err(err),
                }
            })?;
        let kind = args
            .kind
            .unwrap_or_else(|| super::preview_ready::infer_kind(&command));
        let target = args
            .target
            .map(|target| target.trim().to_owned())
            .filter(|target| !target.is_empty())
            .or_else(|| super::preview_ready::infer_target(kind, &command));
        let for_user = args.purpose == Some(crate::tools::PreviewPurpose::User);
        let mut env = args.env.unwrap_or_default();
        // As given, for a start again: what Brigadier adds is added again then.
        let given_env = env.clone();
        // An app runs in the background: the user's work keeps the focus (#142).
        if kind == PreviewKind::App {
            env.entry(brigadier_sandbox::BACKGROUND_LAUNCH_ENV.into())
                .or_insert_with(|| "1".into());
        }
        let installed = brigadier_sandbox::default_data_dir().ok();
        let protected = [Some(self.data_dir.clone()), installed]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        check_env(&env, &workdir, &protected)?;
        check_own_repo(&workspace, &env, &command)?;
        let name = args
            .name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| short_name(&command));

        let starting = self.previews.starting.clone().lock_owned().await;
        if self.previews.stops_of(id) != stops {
            return Err(Error::Invalid(
                "the session's previews were stopped while this one was starting, so it was \
                 not started"
                    .into(),
            ));
        }
        // A task whose folder is gone has no worker any more ([`Self::forget_task_previews`]).
        if let Some(task) = &task
            && (self.previews.task_ended(task) || self.existing_task_live(task).is_none())
        {
            return Err(Error::Invalid(
                "your task has ended and its folder is going, so the preview was not started"
                    .into(),
            ));
        }
        let board = self.core.board(id).await?;
        let preview_id = match again {
            // Started again: only while it is still ended (two clicks start it once).
            Some(again) => {
                if self.previews.get(id, &again).is_some()
                    || board
                        .previews
                        .get(&again)
                        .is_none_or(|preview| preview.state.is_live())
                {
                    return Err(Error::Invalid(format!("{again} is running already")));
                }
                again
            }
            None => format!("preview-{}", board.preview_number + 1),
        };
        // The request it belongs to: started again, its own; a worker's, its task's; the
        // thread's, its running turn's.
        let request_id = if let Some(own) = board.previews.get(&preview_id) {
            own.request_id.clone()
        } else if let Some(task) = &task {
            board
                .tasks
                .get(task)
                .and_then(|task| task.request_id.clone())
        } else {
            match self.conv(id) {
                Ok(conv) => conv.running_request().await,
                Err(_) => None,
            }
        };
        let owner = preview_owner(id);
        let ledger = self.runtime.ledger();
        let dir = self.owned_dir(LOG_AREA, &id.0);
        ledger
            .record(
                &owner,
                Artifact::ScratchDir {
                    path: dir.to_string_lossy().into_owned(),
                },
            )
            .await?;
        let mut spec = self.run_spec(&workdir, &script(&command, &env));
        brigadier_providers::cli::apply_session_env(
            &mut spec,
            &super::workers::worker_env(&scratch),
            &[brigadier_sandbox::DATA_DIR_ENV.to_owned()],
        );
        let log = dir.join(format!("{preview_id}.log"));
        let file = {
            let (dir, log) = (dir.clone(), log.clone());
            blocking(move || {
                std::fs::create_dir_all(&dir).map_err(|err| io_error(&dir, &err))?;
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log)
                    .map_err(|err| io_error(&log, &err))
            })
            .await?
        };
        let platform = self.runtime.platform().clone();
        let mut process = tokio::process::Command::from(platform.processes().piped_command(&spec));
        process
            .stdin(Stdio::null())
            .stdout(file.try_clone().map_err(|err| io_error(&log, &err))?)
            .stderr(file);
        let child = process
            .spawn()
            .map_err(|err| Error::Invalid(format!("could not start the preview: {err}")))?;
        let pid = child
            .id()
            .ok_or_else(|| Error::Invalid("the preview ended before it started".into()))?;
        // A worker's CLI ending sweeps its folder, where its preview runs: spare it till it ends.
        let spared = brigadier_providers::process::spare(pid);
        let artifact = Artifact::Process {
            pid,
            started_at_ms: platform.processes().start_time_ms(pid).ok(),
        };
        let preview = Preview {
            id: preview_id.clone(),
            conversation_id: id.clone(),
            name: name.clone(),
            command,
            workdir: workdir.to_string_lossy().into_owned(),
            workspace: workspace.to_string_lossy().into_owned(),
            pid: Some(pid),
            state: PreviewState::Starting,
            started_at_ms: now_ms(),
            ended_at_ms: None,
            log: None,
            url: None,
            kind,
            task_id: task.clone(),
            building: false,
            env: given_env,
            target: target.clone(),
            pages: None,
            request_id,
        };
        let live = Arc::new(LivePreview {
            conversation: id.clone(),
            id: preview_id.clone(),
            task: task.clone(),
            for_user,
            pid,
            kind,
            workspace,
            pdf: target
                .filter(|_| kind == PreviewKind::Pdf)
                .map(|file| workdir.join(file)),
            log,
            ready: CancellationToken::new(),
            reason: Mutex::default(),
            stop: CancellationToken::new(),
            ended: CancellationToken::new(),
            snapshot: Mutex::default(),
            paused_members: Mutex::default(),
            updating: tokio::sync::Mutex::default(),
            start_told: Mutex::default(),
        });
        self.previews
            .lock()
            .insert((id.clone(), preview_id.clone()), live.clone());
        // From here the preview belongs to a task of its own, so a cancelled call can't leave
        // it running unwatched: it records the process and the start, then watches it.
        let (recorded_tx, recorded_rx) = tokio::sync::oneshot::channel();
        let this = self.arc();
        let (watched, owner_id) = (live.clone(), id.clone());
        self.spawn(async move {
            if let Err(err) = this.runtime.ledger().record(&owner, artifact.clone()).await {
                tracing::warn!(conversation = %owner_id, error = %err, "could not record a preview's process");
            }
            let updating = watched.updating.lock().await;
            let recorded = this.record_preview(preview).await;
            drop(updating);
            if recorded.is_err() {
                watched.ask_to_stop("it could not be recorded");
            }
            drop(starting);
            let _ = recorded_tx.send(recorded);
            this.spawn(this.clone().watch_ready(watched.clone()));
            this.watch_preview(watched, child, artifact).await;
            drop(spared);
        });
        let recorded = recorded_rx.await.unwrap_or_else(|_| {
            Err(Error::Invalid(
                "the preview's start was not recorded".into(),
            ))
        });
        if let Err(err) = recorded {
            live.ended.cancelled().await;
            return Err(err);
        }
        tracing::info!(conversation = %id, preview = %preview_id, pid, workdir = %workdir.display(), "preview started");
        let ended = tokio::time::timeout(START_WATCH, async {
            tokio::select! {
                () = live.ended.cancelled() => true,
                () = live.ready.cancelled() => false,
            }
        })
        .await
        .unwrap_or(false);
        let ended_at_once = ended || {
            let mut told = live.start_told.lock().unwrap_or_else(|p| p.into_inner());
            told.answered = !told.ending;
            told.ending
        };
        if ended_at_once {
            live.ended.cancelled().await;
            return self
                .preview_log_for(
                    id,
                    Some(&preview_id),
                    Some(START_TAIL),
                    None,
                    task.is_some(),
                )
                .await
                .map(|log| format!("{preview_id} ended at once.\n{log}"));
        }
        let output = self
            .preview_log_for(
                id,
                Some(&preview_id),
                Some(START_TAIL),
                None,
                task.is_some(),
            )
            .await
            .unwrap_or_default();
        let state = if live.ready.is_cancelled() {
            "is running".to_owned()
        } else {
            format!(
                "is starting, not ready yet: it reads Starting until {}. Don't tell the user it \
                 runs before preview_log says it is running. To wait for it, call preview_log \
                 with wait_secs (up to 300), which answers the moment it runs or ends; never \
                 sleep in a command for it",
                waits_for(kind)
            )
        };
        let app = match kind {
            PreviewKind::App => {
                " It is a desktop app: it opens its own window and gets no URL, so never send \
                 the user to an address it prints."
            }
            PreviewKind::Simulator => {
                " It runs in an iOS simulator, which the user sees in Simulator: it gets no URL."
            }
            PreviewKind::Pdf => {
                " It renders a PDF: the user opens that file, not an address; it is ready once \
                 the file is there."
            }
            PreviewKind::Test => {
                " It is a test run: it ends with its result, which the user reads in its log; \
                 attach_proof its output with kind \"test\" when it proves the change."
            }
            PreviewKind::Web => "",
        };
        let lifetime = if task.is_some() {
            "It keeps running after you report, until you call stop_preview, and stops when the \
             user presses Stop, when the session closes, when Brigadier quits and when your task \
             ends and its folder is removed: a preview you show the user lasts only while your \
             task is alive"
        } else if for_user {
            "It keeps running across your turns and after the session's work merges, until you \
             call stop_preview, and stops when the session is archived or deleted, when your \
             workspace changes, when the user presses Stop and when Brigadier quits"
        } else {
            "It keeps running across your turns until you call stop_preview, and stops when the \
             session is merged, archived or deleted, when your workspace changes, when the user \
             presses Stop and when Brigadier quits"
        };
        Ok(format!(
            "{preview_id} \"{name}\" {state} (pid {pid}, its own process group) in {}.{app} \
             {lifetime}. Its output so far:\n{output}",
            workdir.display()
        ))
    }

    /// Waits for a preview to end or to be stopped, then records its end.
    async fn watch_preview(
        self: Arc<Self>,
        live: Arc<LivePreview>,
        mut child: tokio::process::Child,
        artifact: Artifact,
    ) {
        let platform = self.runtime.platform().clone();
        let state = tokio::select! {
            status = child.wait() => match status {
                Ok(status) => PreviewState::Exited {
                    code: status.code(),
                    status: super::run::exit_status(status),
                },
                Err(err) => PreviewState::Stopped { reason: format!("lost its process: {err}") },
            },
            () = live.stop.cancelled() => {
                // Serialize with Pause: once stop is requested no later pause can win.
                let updating = live.updating.lock().await;
                let asked = terminate_group(&*platform, &live);
                drop(updating);
                let grace = tokio::time::Instant::now() + STOP_GRACE;
                if tokio::time::timeout_at(grace, child.wait()).await.is_err() {
                    // It didn't go: it and everything it started are killed.
                    let _ = platform.processes().kill_tree(live.pid);
                } else {
                    // It went, and what is left of its group and of what was asked to stop
                    // outside it is killed below: they get the rest of the grace to quit.
                    while tokio::time::Instant::now() < grace
                        && (group_alive(live.pid)
                            || asked
                                .iter()
                                .any(|proc| crate::machine::still(&*platform, *proc)))
                    {
                        tokio::time::sleep(STOP_POLL).await;
                    }
                }
                let _ = child.wait().await;
                PreviewState::Stopped { reason: live.reason() }
            }
        };
        live.stop.cancel();
        let updating = live.updating.lock().await;
        // Reaped: what it left in its group goes too.
        let _ = platform.processes().kill_group(live.pid);
        // A paused descendant may have left the group and lost its parent. Its saved
        // identity still belongs to this preview, so it must not stay suspended forever.
        let members = live
            .paused_members
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for member in members {
            if crate::machine::still(&*platform, member) {
                let _ = platform.processes().kill_tree(member.pid);
            }
            if crate::machine::gone(&*platform, member) {
                self.runtime
                    .ledger()
                    .forget(&preview_owner(&live.conversation), process_artifact(member))
                    .await;
            }
        }
        self.runtime
            .ledger()
            .forget(&preview_owner(&live.conversation), artifact)
            .await;
        drop(updating);
        self.end_preview(&live, state).await;
        self.previews
            .lock()
            .remove(&(live.conversation.clone(), live.id.clone()));
        live.ended.cancel();
    }

    /// Checks a starting preview for readiness every [`READY_POLL`] until it is ready (then
    /// records it as running) or ends. A paused one isn't checked.
    async fn watch_ready(self: Arc<Self>, live: Arc<LivePreview>) {
        let mut wait = Duration::from_millis(250);
        loop {
            tokio::select! {
                () = live.ended.cancelled() => return,
                () = live.stop.cancelled() => return,
                () = tokio::time::sleep(wait) => {}
            }
            wait = READY_POLL;
            let state = match self.core.board(&live.conversation).await {
                Ok(board) => board
                    .previews
                    .get(&live.id)
                    .map(|preview| preview.state.clone()),
                Err(_) => None,
            };
            match state {
                Some(PreviewState::Starting) => {}
                Some(PreviewState::Paused) => continue,
                _ => return,
            }
            let path = live.log.clone();
            let building = blocking(move || Ok(log_building(&read_log_end(&path, BUILD_SCAN))))
                .await
                .unwrap_or(false);
            self.mark_building(&live, building).await;
            // A probe under way (`lsof`, a request) is dropped, and ended, with the preview.
            let url = tokio::select! {
                () = live.ended.cancelled() => return,
                url = self.preview_ready(&live) => url,
            };
            if let Some(url) = url
                && self.mark_ready(&live, url).await
            {
                return;
            }
        }
    }

    /// Whether `live` is ready now: `Some` with the URL it printed (a web preview's), `None`
    /// while it isn't.
    async fn preview_ready(&self, live: &LivePreview) -> Option<Option<String>> {
        use super::preview_ready as ready;
        match live.kind {
            // Nothing of theirs to wait for: running once started.
            PreviewKind::Simulator | PreviewKind::Test => return Some(None),
            PreviewKind::Pdf => {
                let file = live.pdf.clone()?;
                return blocking(move || Ok(file.is_file()))
                    .await
                    .unwrap_or(false)
                    .then_some(None);
            }
            PreviewKind::Web | PreviewKind::App => {}
        }
        let web = live.kind == PreviewKind::Web;
        if !web && !ready::SEES_WINDOWS {
            return Some(None);
        }
        let platform = self.runtime.platform().clone();
        let (group, path) = (live.pid, live.log.clone());
        let (window, url) = blocking(move || {
            let processes = platform.processes();
            let window = ready::window_owners()
                .into_iter()
                .any(|owner| processes.group_of(owner) == Some(group));
            Ok((window, if web { log_url(&path) } else { None }))
        })
        .await
        .ok()?;
        if window {
            return Some(url);
        }
        if !web {
            return None;
        }
        if let Some(url) = &url
            && ready::url_answers(url).await
        {
            return Some(Some(url.clone()));
        }
        for port in ready::listening_ports(group).await {
            if ready::port_answers(port).await {
                // One that printed none (buffered output) is opened on the port that answered (#406).
                return Some(url.or_else(|| Some(port_url(port))));
            }
        }
        None
    }

    /// Records whether a starting preview's log shows a build under way, when that changed.
    async fn mark_building(&self, live: &LivePreview, building: bool) {
        let _updating = live.updating.lock().await;
        let preview = match self.core.board(&live.conversation).await {
            Ok(board) => board.previews.get(&live.id).cloned(),
            Err(_) => None,
        };
        let Some(mut preview) = preview.filter(|preview| {
            preview.state == PreviewState::Starting && preview.building != building
        }) else {
            return;
        };
        preview.building = building;
        if let Err(err) = self.record_preview(preview).await {
            tracing::warn!(conversation = %live.conversation, preview = %live.id, error = %err, "could not record a preview's build");
        }
    }

    /// Records a starting preview as running (with the URL it printed, for a web preview), and
    /// only then says it is ready, so `start_preview` reads the recorded state. False when it
    /// couldn't be recorded (it is checked again).
    async fn mark_ready(&self, live: &LivePreview, url: Option<String>) -> bool {
        let pages = pdf_pages(live).await;
        let _updating = live.updating.lock().await;
        let preview = match self.core.board(&live.conversation).await {
            Ok(board) => board.previews.get(&live.id).cloned(),
            Err(_) => None,
        };
        // A paused one resumes as running.
        let Some(mut preview) = preview.filter(|preview| preview.state == PreviewState::Starting)
        else {
            live.ready.cancel();
            return true;
        };
        tracing::info!(conversation = %live.conversation, preview = %live.id, "preview ready");
        preview.state = PreviewState::Running;
        preview.building = false;
        if live.kind == PreviewKind::Web {
            preview.url = preview.url.or(url);
        }
        preview.pages = pages;
        if let Err(err) = self.record_preview(preview).await {
            tracing::warn!(conversation = %live.conversation, preview = %live.id, error = %err, "could not record a preview as running");
            return false;
        }
        live.ready.cancel();
        true
    }

    /// A worker's preview ended on its own (`told`): its worker hears it while it works, as it
    /// may start it again in its own folder; else the thread only hears of it, as a preview of
    /// another folder than its own, never one to start again itself.
    async fn tell_worker_preview_end(
        &self,
        id: &ConversationId,
        task: &TaskId,
        preview: &Preview,
        told: &str,
    ) {
        let working = match self.existing_task_live(task) {
            Some(worker) => worker.busy().await,
            None => false,
        };
        let task = self.task_by_id(id, task).await.ok();
        if working && let Some(task) = &task {
            let note = format!(
                "[preview] Your {} \"{}\" {told}; it is not running now.",
                preview.id, preview.name
            );
            let rounds = task.rework_rounds;
            match self.tell_worker(id, task, note, "Brigadier").await {
                // A note is no rework round.
                Ok(_) => {
                    let _ = self
                        .update_task(id, &task.id, |t| {
                            t.rework_rounds = t.rework_rounds.min(rounds);
                        })
                        .await;
                    return;
                }
                Err(err) => {
                    tracing::info!(task = %task.id, error = %err, "a worker took no note that its preview ended");
                }
            }
        }
        if let Ok(conv) = self.conv(id) {
            let whose = task.map_or_else(
                || "a worker's".to_owned(),
                |task| format!("task-{}'s", task.number),
            );
            conv.note(format!(
                "[preview] {} \"{}\", {whose} preview in its own folder, {told}; it is not running now.",
                preview.id, preview.name
            ))
            .await;
        }
    }

    /// Records how a preview ended, with its whole log stored, and removes its log file.
    async fn end_preview(&self, live: &LivePreview, state: PreviewState) {
        let id = &live.conversation;
        tracing::info!(conversation = %id, preview = %live.id, state = ?state, "preview ended");
        let status = match &state {
            PreviewState::Exited { status, .. } => status.clone(),
            PreviewState::Stopped { reason } => format!("stopped: {reason}"),
            PreviewState::Starting => "starting".into(),
            PreviewState::Running => "running".into(),
            PreviewState::Paused => "paused".into(),
        };
        let log = self.store_log(live, &status).await;
        let path = live.log.clone();
        let url = match live.kind {
            PreviewKind::Web => blocking(move || Ok(log_url(&path))).await.ok().flatten(),
            PreviewKind::App | PreviewKind::Simulator | PreviewKind::Pdf | PreviewKind::Test => {
                None
            }
        };
        // A rendered PDF's pages as its render left them.
        let pages = pdf_pages(live).await;
        let _updating = live.updating.lock().await;
        let preview = match self.core.board(id).await {
            Ok(board) => board.previews.get(&live.id).cloned(),
            Err(_) => None,
        };
        if let Some(mut preview) = preview {
            // Natural exits stay in the list. User stops send their note after removal,
            // when the ID is no longer available and the stored log is ready.
            // A test run or a PDF's render that ends well has done what it was for.
            let finished = matches!(live.kind, PreviewKind::Test | PreviewKind::Pdf)
                && matches!(state, PreviewState::Exited { code: Some(0), .. });
            let told = match &state {
                PreviewState::Exited { status, .. } if finished => {
                    Some(format!("finished ({status})"))
                }
                PreviewState::Exited { status, .. } => Some(format!("ended on its own ({status})")),
                _ => None,
            };
            // An end on its own after the start answered is a failure the thread reports in
            // the same turn (SESSION-FIXES-PLAN.md Q9): it hears it at once. A worker's
            // preview is the worker's to watch ([`Self::tell_worker_preview_end`]).
            let answered = {
                let mut start = live.start_told.lock().unwrap_or_else(|p| p.into_inner());
                start.ending = true;
                start.answered
            };
            if let (PreviewState::Exited { status, .. }, true, None, false) =
                (&state, answered, &live.task, finished)
            {
                let path = live.log.clone();
                let last = blocking(move || Ok(read_log(&path).0))
                    .await
                    .map(|bytes| last_line(&bytes))
                    .unwrap_or_default();
                self.deliver(
                    id,
                    super::conversation::Envelope {
                        kind: crate::work::InjectionKind::Report,
                        label: format!("preview {}", preview.id),
                        task_id: None,
                        text: format!(
                            "[preview ended: {status}] {} \"{}\" stopped on its own; it is not running now.{}\nTell the user in this turn what failed, with its key error line, and what you do about it, unless you already told them.",
                            preview.id,
                            preview.name,
                            if last.is_empty() {
                                String::new()
                            } else {
                                format!(" Its last output line: {last}")
                            },
                        ),
                    },
                )
                .await;
            } else if let (Some(told), Some(task)) = (&told, &live.task) {
                self.tell_worker_preview_end(id, task, &preview, told).await;
            } else if let (Some(told), Ok(conv)) = (told, self.conv(id)) {
                conv.note(format!(
                    "[preview] {} \"{}\" {told}; it is not running now. Start it again if the user wants it.",
                    preview.id, preview.name
                ))
                .await;
            }
            preview.url = known_url(preview.url, url);
            preview.pages = pages;
            preview.state = state;
            preview.building = false;
            preview.ended_at_ms = Some(now_ms());
            if log.is_some() {
                preview.log = log;
            }
            if let Err(err) = self.record_preview(preview).await {
                tracing::warn!(conversation = %id, preview = %live.id, error = %err, "could not record a preview's end");
            }
        }
        let path = live.log.clone();
        let _ = blocking(move || {
            match std::fs::remove_file(&path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                    tracing::debug!(path = %path.display(), error = %err, "could not remove a preview's log");
                }
                _ => {}
            }
            Ok(())
        })
        .await;
    }

    /// Stores a running preview's log as it stands, unless it is unchanged since its last
    /// snapshot. Returns its alias (none for an empty log).
    async fn store_log(&self, live: &LivePreview, status: &str) -> Option<String> {
        let path = live.log.clone();
        let (bytes, size) = blocking(move || Ok(read_log(&path))).await.ok()?;
        if bytes.is_empty() {
            return None;
        }
        if let Some((seen, alias)) = live
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            && seen == size
        {
            return Some(alias);
        }
        match self
            .store_output(
                &live.conversation,
                OutputSource::Preview,
                status,
                bytes,
                false,
            )
            .await
        {
            Ok((stored, _)) => {
                *live.snapshot.lock().unwrap_or_else(|p| p.into_inner()) =
                    Some((size, stored.alias.clone()));
                Some(stored.alias)
            }
            Err(err) => {
                tracing::warn!(conversation = %live.conversation, preview = %live.id, error = %err, "could not store a preview's log");
                None
            }
        }
    }

    /// Records `alias` as a running preview's log, on its record as it stands now: if it has
    /// ended meanwhile, its end (with its whole log) stays as recorded.
    pub(crate) async fn record_log(&self, live: &LivePreview, alias: &str) {
        let id = &live.conversation;
        let _updating = live.updating.lock().await;
        let current = match self.core.board(id).await {
            Ok(board) => board.previews.get(&live.id).cloned(),
            Err(_) => None,
        };
        let Some(mut current) = current else {
            return;
        };
        if !current.state.is_live() || current.log.as_deref() == Some(alias) {
            return;
        }
        current.log = Some(alias.to_owned());
        if let Err(err) = self.record_preview(current).await {
            tracing::warn!(conversation = %id, preview = %live.id, error = %err, "could not record a preview's log");
        }
    }

    async fn record_preview(&self, preview: Preview) -> Result<()> {
        let id = preview.conversation_id.clone();
        self.core
            .record_conversation(&id, vec![DomainEvent::PreviewUpdated { preview }])
            .await
            .map(drop)
    }

    /// A worker's `preview_log`: [`Self::preview_log`] of one of its own previews (its latest
    /// by default).
    pub(crate) async fn worker_preview_log(
        &self,
        id: &ConversationId,
        task: &TaskId,
        preview: Option<&str>,
        tail_lines: Option<u32>,
        wait_secs: Option<u32>,
    ) -> Result<String> {
        let board = self.core.board(id).await?;
        let found = match preview.map(str::trim).filter(|wanted| !wanted.is_empty()) {
            Some(wanted) => own_preview(&board, task, wanted)?.id.clone(),
            None => match board
                .sorted_previews()
                .into_iter()
                .rfind(|preview| preview.task_id.as_ref() == Some(task))
            {
                Some(latest) => latest.id,
                None => return Ok("You have started no preview.".into()),
            },
        };
        self.preview_log_for(id, Some(&found), tail_lines, wait_secs, true)
            .await
    }

    /// `preview_log`: the end of a preview's output (the latest preview's by default), with an
    /// `out-<id>` reference to its whole log as it stands now; after up to `wait_secs` for one
    /// that is starting to run or end.
    pub(crate) async fn preview_log(
        &self,
        id: &ConversationId,
        preview: Option<&str>,
        tail_lines: Option<u32>,
        wait_secs: Option<u32>,
    ) -> Result<String> {
        self.preview_log_for(id, preview, tail_lines, wait_secs, false)
            .await
    }

    /// [`Self::preview_log`], for a worker when `worker`: it has no `read_artifact`, so its
    /// reply names no `out-<id>`.
    async fn preview_log_for(
        &self,
        id: &ConversationId,
        preview: Option<&str>,
        tail_lines: Option<u32>,
        wait_secs: Option<u32>,
        worker: bool,
    ) -> Result<String> {
        let board = self.core.board(id).await?;
        let mut found = match preview.map(str::trim).filter(|wanted| !wanted.is_empty()) {
            Some(wanted) => board.previews.get(wanted).cloned().ok_or_else(|| {
                Error::Invalid(format!(
                    "there is no {wanted} in this session{}",
                    known_ids(&board.sorted_previews())
                ))
            })?,
            None => match board.sorted_previews().pop() {
                Some(latest) => latest,
                None => return Ok("No preview has been started in this session.".into()),
            },
        };
        // A wait for one that is starting (a build under way) ends the moment it runs or ends,
        // so a caller never sleeps blind past it (#510).
        if let Some(secs) = wait_secs.filter(|secs| *secs > 0)
            && found.state == PreviewState::Starting
            && let Some(live) = self.previews.get(id, &found.id)
        {
            let wait = Duration::from_secs(u64::from(secs.min(WAIT_MAX_SECS)));
            let _ = tokio::time::timeout(wait, async {
                tokio::select! {
                    () = live.ready.cancelled() => {}
                    () = live.ended.cancelled() => {}
                }
            })
            .await;
            if let Some(now) = self.core.board(id).await?.previews.get(&found.id) {
                found = now.clone();
            }
        }
        let lines = tail_lines.unwrap_or(TAIL_DEFAULT).clamp(1, TAIL_MAX);
        let (bytes, alias) = match self.previews.get(id, &found.id) {
            Some(live) => {
                let alias = self.store_log(&live, "running").await;
                if let Some(alias) = &alias {
                    self.record_log(&live, alias).await;
                }
                let path = live.log.clone();
                let (bytes, _) = blocking(move || Ok(read_log(&path))).await?;
                (bytes, alias)
            }
            None => {
                let stored = found
                    .log
                    .as_ref()
                    .and_then(|alias| board.outputs.get(alias).map(|output| (alias, output)));
                match stored {
                    Some((alias, output)) => {
                        let (bytes, _) = self
                            .core
                            .read_blob_range(output.blob.clone(), 0, u32::MAX)
                            .await?;
                        (bytes, Some(alias.clone()))
                    }
                    None => (Vec::new(), None),
                }
            }
        };
        let total_lines = count_lines(&bytes);
        let reference = match &alias {
            Some(_) if worker => format!("{total_lines} lines, {} bytes in all", bytes.len()),
            Some(alias) => format!(
                "full log: read_artifact {alias}, {total_lines} lines, {} bytes",
                bytes.len()
            ),
            None => "no output yet".into(),
        };
        let tail = tail(&bytes, lines);
        Ok(format!(
            "[{} \"{}\" {}; {reference}]\n{tail}",
            found.id,
            found.name,
            state_words(&found)
        ))
    }

    /// The UI reads a bounded tail without storing a fresh artifact on every poll.
    pub async fn preview_log_tail(
        &self,
        id: ConversationId,
        preview_id: String,
        lines: u32,
    ) -> Result<(String, Option<String>)> {
        let live = self.previews.get(&id, &preview_id);
        // URL discovery can read disk without blocking the executor or holding up Pause.
        // Once discovered it is persisted, so subsequent polls need only read the tail.
        let known = self.core.board(&id).await?;
        let url = match (&live, known.previews.get(&preview_id)) {
            (Some(live), Some(preview))
                if preview.url.as_deref().is_none_or(guessed)
                    && preview.kind == PreviewKind::Web =>
            {
                let path = live.log.clone();
                blocking(move || Ok(log_url(&path))).await?
            }
            _ => None,
        };
        // End recording and file removal cannot race the live read.
        let _updating = match &live {
            Some(live) => Some(live.updating.lock().await),
            None => None,
        };
        let board = self.core.board(&id).await?;
        let mut preview =
            board.previews.get(&preview_id).cloned().ok_or_else(|| {
                Error::Invalid(format!("there is no {preview_id} in this session"))
            })?;
        let found = known_url(preview.url.clone(), url);
        if preview.url != found {
            preview.url = found;
            self.record_preview(preview.clone()).await?;
        }
        let bytes = if preview.state.is_live()
            && let Some(live) = &live
        {
            let path = live.log.clone();
            blocking(move || {
                let mut file = std::fs::File::open(&path).map_err(|err| io_error(&path, &err))?;
                let size = file.metadata().map_err(|err| io_error(&path, &err))?.len();
                file.seek(std::io::SeekFrom::Start(
                    size.saturating_sub(TAIL_BYTES_MAX as u64),
                ))
                .map_err(|err| io_error(&path, &err))?;
                let mut bytes = Vec::new();
                file.take(TAIL_BYTES_MAX as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|err| io_error(&path, &err))?;
                Ok(bytes)
            })
            .await?
        } else if let Some(output) = preview
            .log
            .as_ref()
            .and_then(|alias| board.outputs.get(alias))
        {
            self.core
                .read_blob_range(
                    output.blob.clone(),
                    output.bytes.saturating_sub(TAIL_BYTES_MAX as u64),
                    TAIL_BYTES_MAX as u32,
                )
                .await?
                .0
        } else {
            Vec::new()
        };
        Ok((tail(&bytes, lines.clamp(1, TAIL_MAX)), preview.url))
    }

    /// Pausing keeps the live process and its cleanup owner, but does not count as running.
    pub async fn pause_preview(&self, id: ConversationId, preview_id: String) -> Result<()> {
        self.set_preview_paused(id, preview_id, true).await
    }

    pub async fn resume_preview(&self, id: ConversationId, preview_id: String) -> Result<()> {
        self.set_preview_paused(id, preview_id, false).await
    }

    async fn set_preview_paused(
        &self,
        id: ConversationId,
        preview_id: String,
        paused: bool,
    ) -> Result<()> {
        let live = self
            .previews
            .get(&id, &preview_id)
            .ok_or_else(|| Error::Invalid("the preview has ended".into()))?;
        let _updating = live.updating.lock().await;
        let mut preview = self
            .core
            .board(&id)
            .await?
            .previews
            .get(&preview_id)
            .cloned()
            .ok_or_else(|| Error::Invalid("the preview has ended".into()))?;
        if live.stop.is_cancelled() || live.ended.is_cancelled() || !preview.state.is_live() {
            return Err(Error::Invalid(
                "the preview is stopping or has ended".into(),
            ));
        }
        if (preview.state == PreviewState::Paused) == paused {
            return Ok(());
        }
        if let Err(err) = self.signal_preview(&live, paused).await {
            // A failed ledger write or signal may follow a partially suspended tree.
            if paused {
                let _ = self.signal_preview(&live, false).await;
            }
            return Err(err);
        }
        preview.state = if paused {
            PreviewState::Paused
        } else if live.ready.is_cancelled() {
            PreviewState::Running
        } else {
            PreviewState::Starting
        };
        if let Err(err) = self.record_preview(preview).await {
            // Never leave an unrecorded suspended process behind.
            let _ = self.signal_preview(&live, !paused).await;
            return Err(err);
        }
        if let Ok(conv) = self.conv(&id) {
            conv.note(format!(
                "[preview] {preview_id} was {} by the user.",
                if paused { "paused" } else { "resumed" }
            ))
            .await;
        }
        Ok(())
    }

    async fn signal_preview(&self, live: &LivePreview, paused: bool) -> Result<()> {
        self.signal_preview_on(live, paused, self.runtime.platform())
            .await
    }

    async fn signal_preview_on(
        &self,
        live: &LivePreview,
        paused: bool,
        platform: &Arc<dyn brigadier_sandbox::Platform>,
    ) -> Result<()> {
        if let Some(root) = crate::machine::proc_of(&**platform, live.pid) {
            if paused {
                // Record each identity durably before suspending it. Parents stop before
                // children, so a child cannot fork past the walk. A restart's ledger sweep
                // can then kill even a detached member whose original parent has died.
                let owner = preview_owner(&live.conversation);
                let processes = platform.processes();
                // Members from an earlier pause may have lost the parent that led to them,
                // and a detached one is beyond the group signal: walk from each of them too.
                // One that has ended or whose PID was reused is not ours any more: it is
                // dropped, and its ledger record with it, as the stop path forgets them. One
                // whose identity cannot be checked now is not walked, but stays: the record
                // only helps if its identity becomes readable again, so the stop path or a
                // restart's ledger sweep can end it then.
                let (known, dropped): (Vec<_>, Vec<_>) = {
                    let mut members = live
                        .paused_members
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    let (dropped, kept) = std::mem::take(&mut *members)
                        .into_iter()
                        .partition(|member| crate::machine::gone(&**platform, *member));
                    *members = kept;
                    let known = members
                        .iter()
                        .copied()
                        .filter(|member| crate::machine::still(&**platform, *member))
                        .collect();
                    (known, dropped)
                };
                for member in dropped {
                    self.runtime
                        .ledger()
                        .forget(&owner, process_artifact(member))
                        .await;
                }
                let mut queue: Vec<u32> = known.iter().map(|member| member.pid).collect();
                queue.push(root.pid);
                let mut visited = std::collections::HashSet::new();
                while let Some(pid) = queue.pop() {
                    if !visited.insert(pid) || visited.len() > 2_000 {
                        continue;
                    }
                    let Some(member) = known
                        .iter()
                        .find(|member| member.pid == pid)
                        .copied()
                        .or_else(|| crate::machine::proc_of(&**platform, pid))
                    else {
                        continue;
                    };
                    if !crate::machine::still(&**platform, member) {
                        if !crate::machine::gone(&**platform, member) {
                            continue;
                        }
                        let tracked = {
                            let mut members = live
                                .paused_members
                                .lock()
                                .unwrap_or_else(|p| p.into_inner());
                            let before = members.len();
                            members.retain(|tracked| *tracked != member);
                            members.len() != before
                        };
                        if tracked {
                            self.runtime
                                .ledger()
                                .forget(&owner, process_artifact(member))
                                .await;
                        }
                        continue;
                    }
                    self.runtime
                        .ledger()
                        .record(&owner, process_artifact(member))
                        .await?;
                    {
                        let mut members = live
                            .paused_members
                            .lock()
                            .unwrap_or_else(|p| p.into_inner());
                        if !members.contains(&member) {
                            members.push(member);
                        }
                    }
                    if crate::machine::still(&**platform, member) {
                        processes
                            .suspend(pid)
                            .map_err(|err| Error::Invalid(err.to_string()))?;
                        queue.extend(processes.children(pid).unwrap_or_default());
                    }
                }
            } else {
                let members = live
                    .paused_members
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                crate::machine::continue_tree(&**platform, root, &members);
            }
        }
        signal_preview_group(live.pid, paused)
    }

    /// Only ended rows go; stored logs may still be referenced by the thread.
    pub async fn clear_previews(&self, id: ConversationId) -> Result<()> {
        let board = self.core.board(&id).await?;
        let ids = board
            .previews
            .values()
            .filter(|preview| !preview.state.is_live())
            .map(|preview| preview.id.clone())
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            let cleared = ids.join(", ");
            let logs = ids
                .iter()
                .filter_map(|id| {
                    board.previews[id]
                        .log
                        .as_ref()
                        .map(|alias| format!("{id}: read_artifact {alias}"))
                })
                .collect::<Vec<_>>()
                .join("; ");
            self.core
                .record_conversation(
                    &id,
                    vec![DomainEvent::PreviewsCleared {
                        conversation_id: id.clone(),
                        ids,
                    }],
                )
                .await?;
            if let Ok(conv) = self.conv(&id) {
                conv.note(format!("[preview] The user cleared {cleared}. Their preview IDs are no longer available; stored logs remain readable through read_artifact. {logs}")).await;
            }
        }
        Ok(())
    }

    /// `stop_preview`: stops one preview, or every running one, and removes what it stopped
    /// from the user's list (as their Clear finished would); its stored log stays readable. A
    /// worker (`task`) reaches only its own previews.
    pub(crate) async fn stop_preview_tool(
        &self,
        id: &ConversationId,
        preview: Option<&str>,
        task: Option<&TaskId>,
    ) -> Result<String> {
        let reason = if task.is_some() {
            "stopped by its worker"
        } else {
            "stopped by the thread"
        };
        let stopped = self.stop_previews_of(id, preview, task, reason).await?;
        if stopped.is_empty() {
            return Ok("No preview is running.".into());
        }
        self.remove_stopped_previews(id, &stopped).await?;
        Ok(stopped
            .iter()
            .map(|preview| {
                // A worker has no read_artifact.
                let log = preview
                    .log
                    .as_ref()
                    .filter(|_| task.is_none())
                    .map(|alias| format!(", its log: read_artifact {alias}"))
                    .unwrap_or_default();
                format!(
                    "{} \"{}\": {}; removed from the user's list{log}",
                    preview.id,
                    preview.name,
                    state_words(preview)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Records removal only after the stop has durably recorded each preview's end.
    async fn remove_stopped_previews(
        &self,
        id: &ConversationId,
        stopped: &[Preview],
    ) -> Result<()> {
        // One the user resumed or started meanwhile can't be live here: each is recorded as
        // ended before `stop_previews_of` returns, and a live one is never cleared.
        let ids = stopped
            .iter()
            .filter(|preview| !preview.state.is_live())
            .map(|preview| preview.id.clone())
            .collect::<Vec<_>>();
        if !ids.is_empty() {
            self.core
                .record_conversation(
                    id,
                    vec![DomainEvent::PreviewsCleared {
                        conversation_id: id.clone(),
                        ids,
                    }],
                )
                .await?;
        }
        Ok(())
    }

    /// User stops remove only what they stopped. A concurrent natural exit stays inspectable.
    pub(crate) async fn remove_user_stopped_previews(
        &self,
        id: &ConversationId,
        stopped: Vec<Preview>,
    ) -> Result<()> {
        let stopped: Vec<_> = stopped
            .into_iter()
            .filter(|preview| {
                matches!(&preview.state, PreviewState::Stopped { reason } if reason == USER_STOP)
            })
            .collect();
        let removed = self.remove_stopped_previews(id, &stopped).await;
        if let Ok(conv) = self.conv(id) {
            for preview in stopped {
                let log = preview
                    .log
                    .as_ref()
                    .map(|alias| {
                        format!(" Its stored log remains readable: read_artifact {alias}.")
                    })
                    .unwrap_or_default();
                let outcome = if removed.is_ok() {
                    "was stopped by the user and removed; its ID is no longer available"
                } else {
                    "was stopped by the user; it is not running now, but its ID remains listed"
                };
                conv.note(format!(
                    "[preview] {} \"{}\" {outcome}.{log}",
                    preview.id, preview.name
                ))
                .await;
            }
        }
        removed
    }

    /// The user's Stop on a preview row, or Stop all, removes what it stops.
    pub async fn stop_preview(&self, id: ConversationId, preview: Option<String>) -> Result<()> {
        let stopped = if preview.is_none() {
            self.core.board(&id).await?;
            self.stop_previews(&id, USER_STOP).await?
        } else {
            self.stop_previews_of(&id, preview.as_deref(), None, USER_STOP)
                .await?
        };
        self.remove_user_stopped_previews(&id, stopped).await
    }

    /// Stops `preview` (every running one when `None`) of conversation `id` for `reason`, and
    /// returns them as recorded once ended. With `task`, only that worker's previews.
    async fn stop_previews_of(
        &self,
        id: &ConversationId,
        preview: Option<&str>,
        task: Option<&TaskId>,
        reason: &str,
    ) -> Result<Vec<Preview>> {
        let wanted = preview.map(str::trim).filter(|wanted| !wanted.is_empty());
        if let (Some(task), Some(wanted)) = (task, wanted) {
            own_preview(&self.core.board(id).await?, task, wanted)?;
        }
        let live = match wanted {
            Some(wanted) => match self.previews.get(id, wanted) {
                Some(live) => vec![live],
                None => {
                    let board = self.core.board(id).await?;
                    return match board.previews.get(wanted) {
                        Some(ended) => Ok(vec![ended.clone()]),
                        None => Err(Error::Invalid(format!(
                            "there is no {wanted} in this session{}",
                            known_ids(&board.sorted_previews())
                        ))),
                    };
                }
            },
            None => self
                .previews
                .of(id)
                .into_iter()
                .filter(|live| task.is_none() || live.task.as_ref() == task)
                .collect(),
        };
        stop_all(&live, reason).await;
        let board = self.core.board(id).await;
        // A failed final record must not silence a user stop. Use the requests we sent,
        // not just the persisted state; successfully recorded stops are noted after removal.
        if reason == USER_STOP
            && let Ok(conv) = self.conv(id)
        {
            for preview in &live {
                let recorded = board
                    .as_ref()
                    .ok()
                    .and_then(|board| board.previews.get(&preview.id));
                if preview.reason() == USER_STOP
                        && !recorded.is_some_and(|preview| {
                            matches!(&preview.state, PreviewState::Stopped { reason } if reason == USER_STOP)
                        })
                    {
                        conv.note(format!(
                            "[preview] The user requested a stop of {} and its process has ended. Its removal could not be confirmed; do not treat it as running.",
                            preview.id
                        ))
                        .await;
                    }
            }
        }
        let board = board?;
        Ok(live
            .iter()
            .filter_map(|live| board.previews.get(&live.id).cloned())
            .collect())
    }

    /// Stops every running preview of conversation `id` (the user's Stop, a merge, the
    /// session closing).
    pub(crate) async fn stop_previews(
        &self,
        id: &ConversationId,
        reason: &str,
    ) -> Result<Vec<Preview>> {
        // A start already under way is refused, and one that got past that check is waited
        // for, so this stop sees every preview there will be.
        self.previews.count_stop(id);
        drop(self.previews.starting.lock().await);
        self.stop_previews_of(id, None, None, reason).await
    }

    /// Stops the thread's previews of conversation `id` that run in another workspace than
    /// the thread's now (an overnight run started or ended). A worker's preview runs in the
    /// worker's own folder, not the thread's: it stops only once that folder is gone.
    pub(crate) async fn stop_moved_previews(&self, id: &ConversationId) {
        let running = self.previews.of(id);
        if running.is_empty() {
            return;
        }
        let now = self.recorded_workspace(id).map(|workspace| workspace.path);
        let (workers, thread): (Vec<_>, Vec<_>) =
            running.into_iter().partition(|live| live.task.is_some());
        let moved: Vec<_> = thread
            .into_iter()
            .filter(|live| Some(&live.workspace) != now.as_ref())
            .collect();
        let gone: Vec<_> = workers
            .into_iter()
            .filter(|live| !live.workspace.is_dir())
            .collect();
        stop_all(&moved, "the thread's workspace changed").await;
        stop_all(&gone, "its worker's folder was removed").await;
    }

    /// The session's work merged: the thread's previews started to show it stop (#289); one
    /// the user asked for runs on (#335), as does a worker's, in its own folder. Returns what
    /// the merge's result says of the stopped ones (nothing when none ran).
    pub(crate) async fn stop_merged_previews(&self, id: &ConversationId) -> String {
        drop(self.previews.starting.lock().await);
        let live: Vec<_> = self
            .previews
            .of(id)
            .into_iter()
            .filter(|live| live.task.is_none() && !live.for_user)
            .collect();
        stop_all(&live, MERGED_STOP).await;
        let ids: Vec<_> = live.iter().map(|live| live.id.clone()).collect();
        match ids.as_slice() {
            [] => String::new(),
            [one] => format!(" Your preview {one} stopped, as the work it showed is merged."),
            many => format!(
                " Your previews {} stopped, as the work they showed is merged.",
                many.join(", ")
            ),
        }
    }

    /// A task ends and its folder goes: its worker's previews stop first, and a start of its
    /// still under way is refused or waited for, so none outlives the folder it runs in.
    pub(crate) async fn stop_task_previews(&self, id: &ConversationId, task: &TaskId) {
        self.previews
            .ended_tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(task.clone());
        drop(self.previews.starting.lock().await);
        let live: Vec<_> = self
            .previews
            .of(id)
            .into_iter()
            .filter(|live| live.task.as_ref() == Some(task))
            .collect();
        stop_all(&live, "its worker's task ended").await;
    }

    /// Stops `task`'s worker previews for `reason`, after a start of theirs under way, while
    /// the worker carries on.
    pub(crate) async fn stop_worker_previews(
        &self,
        id: &ConversationId,
        task: &TaskId,
        reason: &str,
    ) {
        drop(self.previews.starting.lock().await);
        if let Err(err) = self.stop_previews_of(id, None, Some(task), reason).await {
            tracing::warn!(task = %task, error = %err, "could not stop a worker's previews");
        }
    }

    /// A task's folder is gone, and its worker with it: a start of its taken up from here finds
    /// no worker, so it no longer needs its entry among the ended tasks.
    pub(crate) async fn forget_task_previews(&self, task: &TaskId) {
        let _starting = self.previews.starting.lock().await;
        self.previews
            .ended_tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(task);
    }

    /// Brigadier quits: every preview stops.
    pub(crate) async fn stop_all_previews(&self) {
        stop_all(&self.previews.all(), "Brigadier quit").await;
    }

    /// After a restart: the launch sweep has ended what a previous daemon left running; a
    /// preview still recorded as running reads as stopped, with what its log file kept, and
    /// the conversation's preview folder goes.
    pub(crate) async fn recover_previews(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let dir = self.owned_dir(LOG_AREA, &id.0);
        for preview in board
            .sorted_previews()
            .into_iter()
            .filter(|preview| preview.state.is_live())
        {
            let live = LivePreview {
                conversation: id.clone(),
                id: preview.id.clone(),
                task: preview.task_id.clone(),
                for_user: false,
                pid: preview.pid.unwrap_or_default(),
                kind: preview.kind,
                workspace: PathBuf::from(&preview.workspace),
                pdf: None,
                log: dir.join(format!("{}.log", preview.id)),
                ready: CancellationToken::new(),
                reason: Mutex::new(Some("Brigadier quit".into())),
                stop: CancellationToken::new(),
                ended: CancellationToken::new(),
                snapshot: Mutex::default(),
                paused_members: Mutex::default(),
                updating: tokio::sync::Mutex::default(),
                start_told: Mutex::default(),
            };
            self.end_preview(
                &live,
                PreviewState::Stopped {
                    reason: "Brigadier quit".into(),
                },
            )
            .await;
        }
        let owner = preview_owner(id);
        let ledger = self.runtime.ledger();
        if !ledger.artifacts(&owner).is_empty() {
            let leftovers = ledger.dispose(&owner).await;
            if !leftovers.is_clean() {
                tracing::warn!(
                    owner,
                    ?leftovers,
                    "some preview leftovers will be retried at the next launch"
                );
            }
        }
    }
}

fn process_artifact(member: crate::machine::Proc) -> Artifact {
    Artifact::Process {
        pid: member.pid,
        started_at_ms: Some(member.started_ms as f64),
    }
}

/// A rendered PDF preview's page count, read from its file as it is now; none for another kind
/// of preview or a file that can't be read.
async fn pdf_pages(live: &LivePreview) -> Option<u32> {
    let file = live.pdf.clone()?;
    blocking(move || Ok(crate::documents::pdf_pages(&file)))
        .await
        .ok()
        .flatten()
}

/// Asks each of `live` to stop for `reason`, and waits until each one's end is recorded.
async fn stop_all(live: &[Arc<LivePreview>], reason: &str) {
    for preview in live {
        preview.ask_to_stop(reason);
    }
    for preview in live {
        preview.ended.cancelled().await;
    }
}

/// Previews lead their own group. Processes::suspend targets only one PID, so signal the
/// group explicitly to include the shell's children as well.
fn signal_preview_group(pid: u32, paused: bool) -> Result<()> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let pid = i32::try_from(pid).map_err(|err| Error::Invalid(err.to_string()))?;
        let signal = if paused {
            nix::sys::signal::Signal::SIGSTOP
        } else {
            nix::sys::signal::Signal::SIGCONT
        };
        nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), signal)
            .map_err(|err| Error::Invalid(format!("could not signal the preview: {err}")))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (pid, paused);
        Err(Error::Invalid(
            "pausing previews is unsupported on this platform".into(),
        ))
    }
}

/// Whether any process is left in the group `pid` led.
fn group_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        i32::try_from(pid).is_ok_and(|pid| {
            nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), None).is_ok()
        })
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// SIGTERM to the process group `pid` leads (still the caller's child: not yet reaped), and to
/// what it started or paused outside the group; returns those outside it.
fn terminate_group(
    platform: &dyn brigadier_sandbox::Platform,
    live: &LivePreview,
) -> Vec<crate::machine::Proc> {
    let pid = live.pid;
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(pid) {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
        if let Some(root) = crate::machine::proc_of(platform, live.pid) {
            let members = live
                .paused_members
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut descendants = platform
                .processes()
                .descendants(live.pid)
                .unwrap_or_default();
            descendants.extend(
                members
                    .iter()
                    .filter(|proc| crate::machine::still(platform, **proc))
                    .map(|proc| proc.pid),
            );
            descendants.sort_unstable();
            descendants.dedup();
            let mut outside = Vec::new();
            for child in descendants {
                if platform.processes().group_of(child) != Some(live.pid) {
                    let _ = platform.processes().terminate(child);
                    outside.extend(
                        members
                            .iter()
                            .find(|member| member.pid == child)
                            .copied()
                            .or_else(|| crate::machine::proc_of(platform, child)),
                    );
                }
            }
            // SIGTERM is pending when a stopped process receives SIGCONT, letting it quit.
            let _ = signal_preview_group(live.pid, false);
            crate::machine::continue_tree(platform, root, &members);
            outside
        } else {
            let _ = signal_preview_group(live.pid, false);
            Vec::new()
        }
    }
    #[cfg(not(unix))]
    {
        let _ = platform.processes().terminate(pid);
        Vec::new()
    }
}

/// The folder a preview runs in: the workspace, or `requested` (relative to it), which must be
/// inside it.
/// The port a local URL names: its own, or its scheme's.
fn url_port(url: &str) -> Option<u16> {
    let (scheme, rest) = url.split_once("://")?;
    let host = &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())];
    match host.rsplit_once(':') {
        Some((_, port)) if !port.ends_with(']') => port.parse().ok(),
        _ => match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        },
    }
}

/// The address of a web preview that printed none, from the port that answered (#406).
fn port_url(port: u16) -> String {
    format!("http://localhost:{port}/")
}

/// Whether `url` is only that: the port's root, not an address the server printed.
fn guessed(url: &str) -> bool {
    url_port(url).is_some_and(|port| url == port_url(port))
}

/// A preview's address once its log is read again: the one it printed (`logged`) replaces one
/// guessed from its port, as a server may print its address, route and all, after it answers.
fn known_url(known: Option<String>, logged: Option<String>) -> Option<String> {
    match (known, logged) {
        (Some(known), Some(logged)) if guessed(&known) && url_port(&logged) == url_port(&known) => {
            Some(logged)
        }
        (known, logged) => known.or(logged),
    }
}

/// Whether nothing listens on local `port` now.
async fn port_free(port: u16) -> bool {
    tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .is_ok()
}

fn preview_workdir(requested: Option<&str>, workspace: &Path) -> Result<PathBuf> {
    let wanted = match requested.map(str::trim).filter(|dir| !dir.is_empty()) {
        None => workspace.to_owned(),
        Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir),
        Some(dir) => workspace.join(dir),
    };
    let real = wanted
        .canonicalize()
        .map_err(|err| Error::Invalid(format!("workdir {}: {err}", wanted.display())))?;
    if workspace
        .canonicalize()
        .is_ok_and(|root| real.starts_with(root))
    {
        Ok(real)
    } else {
        Err(Error::Invalid(format!(
            "workdir {} is outside the workspace",
            wanted.display()
        )))
    }
}

/// Checks the variables a preview is given: names a shell takes, and no `BRIGADIER_DATA_DIR`
/// naming one of `protected` (the installed app's data folder, this daemon's) or a folder in
/// one.
fn check_env(env: &BTreeMap<String, String>, workdir: &Path, protected: &[PathBuf]) -> Result<()> {
    for (name, value) in env {
        let valid = name
            .chars()
            .next()
            .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
            && name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric());
        if !valid {
            return Err(Error::Invalid(format!(
                "{name:?} is not an environment variable name"
            )));
        }
        if name == brigadier_sandbox::DATA_DIR_ENV {
            let dir = if Path::new(value).is_absolute() {
                PathBuf::from(value)
            } else {
                workdir.join(value)
            };
            let dir = resolved(&dir);
            // `..` after a symlink names another folder than the text says, so none is taken.
            let climbs = Path::new(value)
                .components()
                .any(|part| part == std::path::Component::ParentDir);
            let temp = resolved(Path::new("/tmp"));
            if value.trim().is_empty()
                || climbs
                || dir == temp
                || !dir.starts_with(&temp)
                || protected.iter().any(|own| {
                    let own = resolved(own);
                    dir.starts_with(&own) || own.starts_with(&dir)
                })
            {
                return Err(Error::Invalid(format!(
                    "BRIGADIER_DATA_DIR must be a scratch folder of the preview's own (under \
                     /tmp), never Brigadier's own data folder; {value:?} is not"
                )));
            }
        }
    }
    Ok(())
}

pub(crate) fn preview_data_owner(id: &ConversationId) -> String {
    format!("preview-data:{id}")
}

fn resolved(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => {
                clean.pop();
            }
            std::path::Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let mut rest = Vec::new();
    let mut base = clean.as_path();
    loop {
        if let Ok(real) = base.canonicalize() {
            return rest.iter().rev().fold(real, |path, part| path.join(part));
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                base = parent;
            }
            _ => return clean,
        }
    }
}

/// In Brigadier's own repository a preview must run with a scratch data folder of its own,
/// and never under the installed app's identity.
fn check_own_repo(workspace: &Path, env: &BTreeMap<String, String>, command: &str) -> Result<()> {
    if command.contains(brigadier_sandbox::APP_ID) {
        return Err(Error::Invalid(format!(
            "a preview never runs as {}: give the app a dev identity (`pnpm tauri dev --config \
             '{{\"identifier\":\"ai.brigadier.<name>\"}}'`)",
            brigadier_sandbox::APP_ID
        )));
    }
    let own = workspace.join("crates/daemon").is_dir()
        && workspace.join("apps/desktop/src-tauri").is_dir();
    if own && !env.contains_key(brigadier_sandbox::DATA_DIR_ENV) {
        return Err(Error::Invalid(
            "this is Brigadier's own repository: set `env.BRIGADIER_DATA_DIR` to a scratch \
             folder (under /tmp) and give the app a dev identity (`pnpm tauri dev --config \
             '{\"identifier\":\"ai.brigadier.<name>\"}'`), never the installed app's"
                .into(),
        ));
    }
    Ok(())
}

/// The shell script that runs `command` with `env` exported first (so a sandbox that filters
/// the environment keeps them).
fn script(command: &str, env: &BTreeMap<String, String>) -> String {
    let mut script = String::new();
    for (name, value) in env {
        script.push_str(&format!("export {name}={}\n", shell_quote(value)));
    }
    script.push_str(command);
    script
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// A command as a name: its first line, at most 48 characters.
fn short_name(command: &str) -> String {
    let line = command.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= 48 {
        return line.to_owned();
    }
    let cut: String = line.chars().take(47).collect();
    format!("{cut}…")
}

/// Startup messages retain the server address even after its tail has scrolled away.
fn log_url(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(16 * 1024).read_to_end(&mut bytes).ok()?;
    preview_url(&bytes)
}

/// A log's text without its terminal escape codes (colours, erased lines).
fn plain_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut plain = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            plain.push(ch);
        }
    }
    plain
}

/// Whether a log shows a build under way: its latest line of build progress (cargo's
/// Compiling, Building and Bundling, a bundler building for production) comes after its latest
/// line saying a build finished (`Finished`, cargo's ``Running `…` ``, `built in`). A dev app
/// built and launched in one command reads Building until then, and Starting after.
fn log_building(bytes: &[u8]) -> bool {
    const PROGRESS: [&str; 9] = [
        "Compiling ",
        "Building ",
        "Bundling ",
        "Checking ",
        "Downloading ",
        "Downloaded ",
        "Locking ",
        "Updating crates.io",
        "Running beforeBuildCommand",
    ];
    const BUNDLER: [&str; 3] = [
        "building for production",
        "transforming",
        "rendering chunks",
    ];
    let mut building = false;
    for line in plain_text(bytes).split(['\n', '\r']) {
        let line = line.trim_start_matches(|c: char| c.is_whitespace() || !c.is_ascii());
        if line.starts_with("Finished")
            || line.starts_with("Running `")
            || line.contains("built in ")
        {
            building = false;
        } else if PROGRESS.iter().any(|start| line.starts_with(start))
            || BUNDLER.iter().any(|words| line.contains(words))
        {
            building = true;
        }
    }
    building
}

/// The last `max` bytes of a log file (none when it can't be read).
fn read_log_end(path: &Path, max: u64) -> Vec<u8> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let size = file.metadata().map(|meta| meta.len()).unwrap_or_default();
    if file
        .seek(std::io::SeekFrom::Start(size.saturating_sub(max)))
        .is_err()
    {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    let _ = file.take(max).read_to_end(&mut bytes);
    bytes
}

/// The first local http(s) address in a log: on localhost or 127.0.0.1, or on every address
/// (`0.0.0.0`, `[::]`), read as localhost's (#406).
fn preview_url(bytes: &[u8]) -> Option<String> {
    let plain = plain_text(bytes);
    for (at, _) in plain.match_indices("http") {
        let candidate = plain[at..]
            .split(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '\"' | '\'' | '`'))
            .next()?
            .trim_end_matches([')', ']', '}', ',', ';', '.']);
        let Some(address) = candidate
            .strip_prefix("http://")
            .or_else(|| candidate.strip_prefix("https://"))
        else {
            continue;
        };
        let authority = address.split(['/', '?', '#']).next().unwrap_or_default();
        let (host, port) = match authority.strip_prefix('[') {
            Some(bracketed) => match bracketed.split_once(']') {
                Some((host, rest)) if rest.is_empty() || rest.starts_with(':') => {
                    (host, rest.strip_prefix(':'))
                }
                _ => continue,
            },
            None => authority
                .split_once(':')
                .map_or((authority, None), |(host, port)| (host, Some(port))),
        };
        let every = matches!(host, "0.0.0.0" | "::");
        if !every && !matches!(host, "localhost" | "127.0.0.1") {
            continue;
        }
        if port.is_some_and(|port| {
            port.is_empty()
                || !port.bytes().all(|b| b.is_ascii_digit())
                || port.parse::<u16>().is_err()
        }) {
            continue;
        }
        if every {
            let scheme = &candidate[..candidate.len() - address.len()];
            let port = port.map(|port| format!(":{port}")).unwrap_or_default();
            return Some(format!(
                "{scheme}localhost{port}{}",
                &address[authority.len()..]
            ));
        }
        return Some(candidate.to_owned());
    }
    None
}

/// A log file's bytes (at most the last [`OUTPUT_MAX_BYTES`], marked when cut) and its size;
/// nothing for a log that isn't there.
fn read_log(path: &Path) -> (Vec<u8>, u64) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return (Vec::new(), 0);
    };
    let size = file.metadata().map(|meta| meta.len()).unwrap_or_default();
    let keep = OUTPUT_MAX_BYTES as u64;
    let mut bytes = Vec::new();
    if size > keep {
        if file.seek(std::io::SeekFrom::Start(size - keep)).is_err() {
            return (Vec::new(), size);
        }
        bytes.extend_from_slice(format!("[… {} earlier bytes not kept]\n", size - keep).as_bytes());
    }
    let _ = file.take(keep).read_to_end(&mut bytes);
    (bytes, size)
}

fn count_lines(bytes: &[u8]) -> usize {
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count();
    newlines + usize::from(!bytes.is_empty() && !bytes.ends_with(b"\n"))
}

/// The last `lines` lines of `bytes`, at most [`TAIL_BYTES_MAX`] bytes of them, marked when
/// lines were left out.
fn tail(bytes: &[u8], lines: u32) -> String {
    let text = String::from_utf8_lossy(bytes);
    let all: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    if text.is_empty() {
        return String::new();
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0;
    for line in all.iter().rev().take(lines as usize) {
        if size + line.len() + 1 > TAIL_BYTES_MAX {
            break;
        }
        size += line.len() + 1;
        kept.push(line);
    }
    kept.reverse();
    let left = all.len() - kept.len();
    let mut out = String::new();
    if left > 0 {
        out.push_str(&format!("[… {left} earlier lines]\n"));
    }
    out.push_str(&kept.join("\n"));
    out
}

/// The log's last line with text, trimmed to 300 characters.
fn last_line(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(300) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_owned(),
    }
}

fn state_words(preview: &Preview) -> String {
    match &preview.state {
        PreviewState::Starting => format!(
            "starting, not ready yet (it reads running once {}){}",
            waits_for(preview.kind),
            preview
                .pid
                .map(|pid| format!(", pid {pid}"))
                .unwrap_or_default()
        ),
        PreviewState::Running => match preview.pid {
            Some(pid) => format!("running, pid {pid}"),
            None => "running".into(),
        },
        PreviewState::Paused => "paused by the user".into(),
        PreviewState::Exited { status, .. } => format!("ended on its own ({status})"),
        PreviewState::Stopped { reason } => format!("stopped ({reason})"),
    }
}

/// What a starting preview of `kind` waits for to be ready.
fn waits_for(kind: PreviewKind) -> &'static str {
    match kind {
        PreviewKind::Web => {
            "the local URL it prints, or a port it listens on, answers an HTTP request, or it \
             opens a window"
        }
        PreviewKind::App => "it opens its window",
        PreviewKind::Simulator | PreviewKind::Test => "it has started",
        PreviewKind::Pdf => "its PDF file is there",
    }
}

/// `wanted`, when it is one of `task`'s previews: a worker reaches only its own.
fn own_preview<'a>(
    board: &'a crate::board::Board,
    task: &TaskId,
    wanted: &str,
) -> Result<&'a Preview> {
    board
        .previews
        .get(wanted)
        .filter(|preview| preview.task_id.as_ref() == Some(task))
        .ok_or_else(|| {
            let own = board
                .sorted_previews()
                .into_iter()
                .filter(|preview| preview.task_id.as_ref() == Some(task))
                .collect::<Vec<_>>();
            Error::Invalid(format!(
                "{wanted} is not one of your previews{}",
                known_ids(&own)
            ))
        })
}

fn known_ids(previews: &[Preview]) -> String {
    if previews.is_empty() {
        return String::new();
    }
    format!(
        " (its previews: {})",
        previews
            .iter()
            .map(|preview| preview.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn io_error(path: &Path, err: &std::io::Error) -> Error {
    Error::Invalid(format!("{}: {err}", path.display()))
}

#[cfg(all(test, unix))]
mod tests {
    /// A URL's port, its own or its scheme's.
    #[test]
    fn a_urls_port_is_its_own_or_its_schemes() {
        assert_eq!(super::url_port("http://localhost:5173/app"), Some(5173));
        assert_eq!(super::url_port("http://127.0.0.1:3000"), Some(3000));
        assert_eq!(super::url_port("http://localhost/"), Some(80));
        assert_eq!(super::url_port("https://example.test?x=1"), Some(443));
        assert_eq!(super::url_port("file:///tmp/a.pdf"), None);
    }

    use super::*;

    /// A dev app built and launched in one command reads Building while its log shows the
    /// build under way, and no longer once the build finished.
    #[test]
    fn a_build_log_reads_building_until_it_finishes() {
        let steps = [
            (
                "> vite build\nvite v6.0.0 building for production...\n",
                true,
            ),
            ("transforming (412) src/main.tsx\n", true),
            ("\u{2713} built in 3.20s\n", false),
            ("Running beforeBuildCommand `pnpm build`\n", true),
            ("   Compiling brigadier-core v0.1.0\n", true),
            (
                "    Finished `dev` profile [unoptimized] target(s) in 41.2s\n",
                false,
            ),
            (
                "    Bundling Brigadier Dev.app (/work/target/debug/bundle)\n",
                true,
            ),
            (
                "    Finished 1 bundle at:\n        /work/Brigadier Dev.app\n",
                false,
            ),
        ];
        let mut log = String::new();
        for (line, building) in steps {
            log.push_str(line);
            assert_eq!(log_building(log.as_bytes()), building, "{log}");
        }
        assert!(!log_building(b"listening on 8123\n"));
        assert!(log_building(b"\x1b[1;32m   Compiling\x1b[0m demo v0.1.0\n"));
    }

    #[tokio::test]
    async fn paused_preview_members_survive_ledger_replay_without_their_parent() {
        use crate::manager::flow::{Flow, Options, Reply, eventually};

        let mut flow = Flow::start(
            "preview-paused-crash",
            Options::default(),
            Arc::new(|_| Box::pin(async { Reply::text("") })),
        )
        .await;
        let platform = flow.manager.runtime.platform().clone();
        let pid_file = flow.repo.join("detached.pid");
        let mut spec = flow.manager.runtime.cli_env().spec(Path::new("python3"));
        spec.args = vec![
            "-c".into(),
            r#"
import os, sys, time
if os.fork() == 0:
    os.setsid()
    with open(sys.argv[1], 'w') as f: f.write(str(os.getpid()))
while True: time.sleep(0.03)
"#
            .into(),
            pid_file.as_os_str().to_owned(),
        ];
        let mut child = platform.processes().piped_command(&spec).spawn().unwrap();
        eventually("the detached preview process", || {
            std::fs::read_to_string(&pid_file).is_ok_and(|pid| !pid.is_empty())
        })
        .await;
        let detached: u32 = std::fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        let live = LivePreview {
            conversation: flow.conversation.clone(),
            id: "preview-1".into(),
            task: None,
            for_user: false,
            pid: child.id(),
            kind: PreviewKind::Web,
            workspace: flow.repo.clone(),
            pdf: None,
            ready: CancellationToken::new(),
            log: flow.repo.join("unused.log"),
            reason: Mutex::default(),
            stop: CancellationToken::new(),
            ended: CancellationToken::new(),
            snapshot: Mutex::default(),
            paused_members: Mutex::default(),
            updating: tokio::sync::Mutex::default(),
            start_told: Mutex::default(),
        };
        flow.manager.signal_preview(&live, true).await.unwrap();
        let owner = preview_owner(&flow.conversation);
        let member = crate::machine::proc_of(&*platform, detached).unwrap();
        assert!(
            flow.manager
                .runtime
                .ledger()
                .artifacts(&owner)
                .contains(&process_artifact(member))
        );
        // Lose both the in-memory membership and the leader, just as a crash can. The
        // detached process is now unreachable through the original recorded root.
        drop(live);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(crate::machine::still(&*platform, member));
        flow.restart().await;
        eventually("the replayed ledger to kill the suspended orphan", || {
            !crate::machine::still(&*platform, member)
        })
        .await;
        assert!(flow.manager.runtime.ledger().artifacts(&owner).is_empty());
        flow.stop().await;
    }

    #[tokio::test]
    async fn pausing_again_stops_a_detached_member_whose_parent_exited() {
        use crate::manager::flow::{Flow, Options, Reply, eventually};

        fn stopped(pid: u32) -> bool {
            let out = std::process::Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().starts_with('T')
        }

        let flow = Flow::start(
            "preview-repause-orphan",
            Options::default(),
            Arc::new(|_| Box::pin(async { Reply::text("") })),
        )
        .await;
        let platform = flow.manager.runtime.platform().clone();
        let parent_file = flow.repo.join("parent.pid");
        let detached_file = flow.repo.join("detached.pid");
        let mut spec = flow.manager.runtime.cli_env().spec(Path::new("python3"));
        // The leader reaps its child, so an exited parent leaves no zombie behind.
        spec.args = vec![
            "-c".into(),
            r#"
import os, sys, time
if os.fork() == 0:
    if os.fork() == 0:
        os.setsid()
        with open(sys.argv[2], 'w') as f: f.write(str(os.getpid()))
    else:
        with open(sys.argv[1], 'w') as f: f.write(str(os.getpid()))
    while True: time.sleep(0.03)
while True:
    try: os.waitpid(-1, os.WNOHANG)
    except ChildProcessError: pass
    time.sleep(0.03)
"#
            .into(),
            parent_file.as_os_str().to_owned(),
            detached_file.as_os_str().to_owned(),
        ];
        /// Kills what the test started however it ends: the grandchild loops forever. Only
        /// a process that still is the one it was, so a reused PID is left alone.
        struct Kill(
            Arc<dyn brigadier_sandbox::Platform>,
            Vec<crate::machine::Proc>,
        );

        impl Drop for Kill {
            fn drop(&mut self) {
                for proc in &self.1 {
                    if crate::machine::still(&*self.0, *proc) {
                        let _ = self.0.processes().kill_tree(proc.pid);
                    }
                }
            }
        }

        let mut child = platform.processes().piped_command(&spec).spawn().unwrap();
        let mut kill = Kill(platform.clone(), Vec::new());
        kill.1
            .extend(crate::machine::proc_of(&*platform, child.id()));
        eventually("the parent and detached preview processes", || {
            [&parent_file, &detached_file]
                .iter()
                .all(|file| std::fs::read_to_string(file).is_ok_and(|pid| !pid.is_empty()))
        })
        .await;
        let parent: u32 = std::fs::read_to_string(&parent_file)
            .unwrap()
            .parse()
            .unwrap();
        kill.1.extend(crate::machine::proc_of(&*platform, parent));
        let detached: u32 = std::fs::read_to_string(&detached_file)
            .unwrap()
            .parse()
            .unwrap();
        kill.1.extend(crate::machine::proc_of(&*platform, detached));
        let parent = crate::machine::proc_of(&*platform, parent).unwrap();
        let live = LivePreview {
            conversation: flow.conversation.clone(),
            id: "preview-1".into(),
            task: None,
            for_user: false,
            pid: child.id(),
            kind: PreviewKind::Web,
            workspace: flow.repo.clone(),
            pdf: None,
            ready: CancellationToken::new(),
            log: flow.repo.join("unused.log"),
            reason: Mutex::default(),
            stop: CancellationToken::new(),
            ended: CancellationToken::new(),
            snapshot: Mutex::default(),
            paused_members: Mutex::default(),
            updating: tokio::sync::Mutex::default(),
            start_told: Mutex::default(),
        };
        flow.manager.signal_preview(&live, true).await.unwrap();
        assert!(stopped(detached));
        let owner = preview_owner(&flow.conversation);
        let recorded = |proc| {
            flow.manager
                .runtime
                .ledger()
                .artifacts(&owner)
                .contains(&process_artifact(proc))
        };
        assert!(recorded(parent));
        flow.manager.signal_preview(&live, false).await.unwrap();
        assert!(!stopped(detached));
        // The leader lives on, but nothing leads from it to the detached process any more.
        platform.processes().terminate(parent.pid).unwrap();
        eventually("the intermediate parent to exit", || {
            !crate::machine::still(&*platform, parent)
        })
        .await;
        flow.manager.signal_preview(&live, true).await.unwrap();
        assert!(stopped(detached));
        // The dead parent is dropped from the members, and its ledger record with it.
        assert!(!recorded(parent));
        assert!(
            live.paused_members
                .lock()
                .unwrap()
                .iter()
                .all(|member| *member != parent)
        );

        flow.manager.signal_preview(&live, false).await.unwrap();
        drop(kill);
        child.wait().unwrap();
        flow.stop().await;
    }

    #[tokio::test]
    async fn failed_identity_checks_keep_paused_members_and_their_ledger_records() {
        use crate::machine::tests::Unreadable;
        use crate::manager::flow::{Flow, Options, Reply};

        let flow = Flow::start(
            "preview-unreadable-member",
            Options::default(),
            Arc::new(|_| Box::pin(async { Reply::text("") })),
        )
        .await;
        let platform = flow.manager.runtime.platform().clone();
        let spec = flow.manager.runtime.cli_env().spec(Path::new("/bin/cat"));
        let mut child = platform.processes().piped_command(&spec).spawn().unwrap();
        let mut detached = platform.processes().piped_command(&spec).spawn().unwrap();
        let member = crate::machine::proc_of(&*platform, detached.id()).unwrap();
        let live = LivePreview {
            conversation: flow.conversation.clone(),
            id: "preview-1".into(),
            task: None,
            for_user: false,
            pid: child.id(),
            kind: PreviewKind::Web,
            workspace: flow.repo.clone(),
            pdf: None,
            ready: CancellationToken::new(),
            log: flow.repo.join("unused.log"),
            reason: Mutex::default(),
            stop: CancellationToken::new(),
            ended: CancellationToken::new(),
            snapshot: Mutex::default(),
            paused_members: Mutex::new(vec![member]),
            updating: tokio::sync::Mutex::default(),
            start_told: Mutex::default(),
        };
        let owner = preview_owner(&flow.conversation);
        let ledger = flow.manager.runtime.ledger();
        ledger
            .record(&owner, process_artifact(member))
            .await
            .unwrap();
        platform.processes().suspend(member.pid).unwrap();
        // Fail either at the initial partition or after it has accepted the member into
        // the walk (gone and still each read its identity once).
        for readable_checks in [0, 2] {
            let unreadable: Arc<dyn brigadier_sandbox::Platform> = Arc::new(Unreadable(
                platform.clone(),
                member.pid,
                readable_checks.into(),
            ));
            let result = flow
                .manager
                .signal_preview_on(&live, true, &unreadable)
                .await;
            assert!(result.is_ok(), "{result:?}");
            assert!(live.paused_members.lock().unwrap().contains(&member));
            assert!(ledger.artifacts(&owner).contains(&process_artifact(member)));
        }
        child.kill().unwrap();
        detached.kill().unwrap();
        child.wait().unwrap();
        detached.wait().unwrap();
        flow.stop().await;
    }

    /// A PID now held by another user's process (launchd), which PROC_PIDTBSDINFO refuses:
    /// the kernel's own start time still shows it is not the member, which is forgotten.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_member_whose_pid_another_user_reused_is_forgotten() {
        use crate::manager::flow::{Flow, Options, Reply};

        let flow = Flow::start(
            "preview-reused-member",
            Options::default(),
            Arc::new(|_| Box::pin(async { Reply::text("") })),
        )
        .await;
        let platform = flow.manager.runtime.platform().clone();
        let spec = flow.manager.runtime.cli_env().spec(Path::new("/bin/cat"));
        let mut child = platform.processes().piped_command(&spec).spawn().unwrap();
        let launchd = crate::machine::proc_of(&*platform, 1).unwrap();
        let member = crate::machine::Proc {
            started_ms: launchd.started_ms + 5_000,
            ..launchd
        };
        let live = LivePreview {
            conversation: flow.conversation.clone(),
            id: "preview-1".into(),
            task: None,
            for_user: false,
            pid: child.id(),
            kind: PreviewKind::Web,
            workspace: flow.repo.clone(),
            pdf: None,
            ready: CancellationToken::new(),
            log: flow.repo.join("unused.log"),
            reason: Mutex::default(),
            stop: CancellationToken::new(),
            ended: CancellationToken::new(),
            snapshot: Mutex::default(),
            paused_members: Mutex::new(vec![member]),
            updating: tokio::sync::Mutex::default(),
            start_told: Mutex::default(),
        };
        let owner = preview_owner(&flow.conversation);
        let ledger = flow.manager.runtime.ledger();
        ledger
            .record(&owner, process_artifact(member))
            .await
            .unwrap();
        let result = flow.manager.signal_preview_on(&live, true, &platform).await;
        assert!(result.is_ok(), "{result:?}");
        assert!(!live.paused_members.lock().unwrap().contains(&member));
        assert!(!ledger.artifacts(&owner).contains(&process_artifact(member)));
        child.kill().unwrap();
        child.wait().unwrap();
        flow.stop().await;
    }

    #[test]
    fn a_printed_address_replaces_one_guessed_from_its_port() {
        let guess = Some(super::port_url(3000));
        let printed = Some("http://localhost:3000/app/".to_owned());
        assert_eq!(super::known_url(guess.clone(), printed.clone()), printed);
        assert_eq!(super::known_url(guess.clone(), None), guess);
        // Another port's, or over an address it printed itself, it is kept.
        let other = Some("http://localhost:4000/app/".to_owned());
        assert_eq!(super::known_url(guess.clone(), other), guess);
        assert_eq!(
            super::known_url(printed.clone(), Some("http://localhost:3000/x".into())),
            printed
        );
        assert_eq!(super::known_url(None, printed.clone()), printed);
    }

    #[test]
    fn preview_urls_strip_color_and_only_accept_local_http_addresses() {
        assert_eq!(
            preview_url(b"Local: \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[0m"),
            Some("http://localhost:5173/".into())
        );
        assert_eq!(
            preview_url(b"(http://127.0.0.1:8080/path?q=1)."),
            Some("http://127.0.0.1:8080/path?q=1".into())
        );
        // A server listening on every address is opened on this machine's (#406).
        assert_eq!(
            preview_url(b"Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ..."),
            Some("http://localhost:8000/".into())
        );
        assert_eq!(
            preview_url(b"Serving HTTP on :: port 8000 (http://[::]:8000/) ..."),
            Some("http://localhost:8000/".into())
        );
        assert_eq!(
            preview_url(b"listening on http://0.0.0.0:3000/app?x=1"),
            Some("http://localhost:3000/app?x=1".into())
        );
        for bad in [
            "http://[::]:nope/",
            "http://0.0.0.0.evil/",
            "https://localhost.evil/",
            "http://127.0.0.1@evil/",
            "http://localhost:nope/",
            "http://localhost:99999/",
            "https://example.com/",
        ] {
            assert_eq!(preview_url(bad.as_bytes()), None, "{bad}");
        }
    }

    #[test]
    fn old_preview_events_load_without_a_url_and_clears_keep_active_rows() {
        let old = serde_json::json!({"type":"previewUpdated", "preview": {
            "id":"preview-3", "conversationId":"c", "name":"web", "command":"serve",
            "workdir":"/work", "workspace":"/work", "state":{"type":"running"}, "startedAtMs":1
        }});
        let event: DomainEvent = serde_json::from_value(old).unwrap();
        let mut board = crate::board::Board::default();
        board.apply(&event, 1);
        assert_eq!(board.previews["preview-3"].url, None);
        assert_eq!(
            board.previews["preview-3"].kind,
            crate::work::PreviewKind::Web
        );
        assert_eq!(board.preview_number, 3);
        let clear = DomainEvent::PreviewsCleared {
            conversation_id: ConversationId("c".into()),
            ids: vec!["preview-3".into()],
        };
        board.apply(&clear, 2);
        assert_eq!(board.previews.len(), 1);
        let mut preview = board.previews["preview-3"].clone();
        preview.state = PreviewState::Paused;
        board.apply(
            &DomainEvent::PreviewUpdated {
                preview: preview.clone(),
            },
            3,
        );
        board.apply(&clear, 4);
        assert_eq!(board.previews.len(), 1);
        preview.state = PreviewState::Exited {
            code: Some(0),
            status: "exit 0".into(),
        };
        board.apply(&DomainEvent::PreviewUpdated { preview }, 5);
        board.apply(&clear, 6);
        assert!(board.previews.is_empty());
        assert_eq!(board.preview_number, 3);
    }

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

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

    #[test]
    fn a_preview_runs_inside_the_workspace_only() {
        let root =
            Temp(std::env::temp_dir().join(format!("brigadier-preview-{}", uuid::Uuid::new_v4())));
        let workspace = root.join("ws");
        std::fs::create_dir_all(workspace.join("web")).unwrap();
        std::fs::create_dir_all(root.join("orch")).unwrap();
        let real = |path: &Path| path.canonicalize().unwrap();
        assert_eq!(preview_workdir(None, &workspace).unwrap(), real(&workspace));
        assert_eq!(
            preview_workdir(Some("web"), &workspace).unwrap(),
            real(&workspace.join("web"))
        );
        // Not even the thread's scratch folder, which hibernation sweeps.
        for outside in ["..", "../orch", "/", "web/../.."] {
            assert!(
                preview_workdir(Some(outside), &workspace).is_err(),
                "{outside}"
            );
        }
    }

    #[test]
    fn a_preview_never_gets_brigadiers_own_data_folder() {
        let workdir = Path::new("/tmp");
        let protected = [PathBuf::from(
            "/Users/x/Library/Application Support/Brigadier",
        )];
        let env = |name: &str, value: &str| BTreeMap::from([(name.to_owned(), value.to_owned())]);
        assert!(check_env(&env("PORT", "4000"), workdir, &protected).is_ok());
        assert!(
            check_env(
                &env("BRIGADIER_DATA_DIR", "/tmp/preview-data"),
                workdir,
                &protected
            )
            .is_ok()
        );
        for value in [
            "/Users/x/Library/Application Support/Brigadier",
            "/Users/x/Library/Application Support/Brigadier/sub",
            "/Users/x/Library",
            "/tmp",
            "/private/tmp",
            "/etc",
            "",
        ] {
            assert!(
                check_env(&env("BRIGADIER_DATA_DIR", value), workdir, &protected).is_err(),
                "{value}"
            );
        }
        // Keep this under /tmp even when macOS's TMPDIR is /var/folders, so these cases
        // exercise protected-folder validation rather than the /tmp requirement.
        let temp = resolved(&std::env::temp_dir());
        let temp = if temp.starts_with(resolved(Path::new("/tmp"))) {
            temp
        } else {
            PathBuf::from("/tmp")
        };
        let root = Temp(temp.join(format!("brigadier-own-{}", uuid::Uuid::new_v4())));
        let own = root.join("data");
        std::fs::create_dir_all(&own).unwrap();
        std::os::unix::fs::symlink(&own, root.join("link")).unwrap();
        let via_link = root.join("link/new/sub");
        assert!(
            check_env(
                &env("BRIGADIER_DATA_DIR", via_link.to_str().unwrap()),
                workdir,
                std::slice::from_ref(&own)
            )
            .is_err()
        );
        // A link to a folder inside a protected one, then `..`: the text names `root/data`'s
        // sibling, the file system names the protected folder itself.
        std::fs::create_dir_all(own.join("inner")).unwrap();
        std::os::unix::fs::symlink(own.join("inner"), root.join("inner-link")).unwrap();
        let climbed = root.join("inner-link/../sub");
        assert!(
            check_env(
                &env("BRIGADIER_DATA_DIR", climbed.to_str().unwrap()),
                workdir,
                std::slice::from_ref(&own)
            )
            .is_err()
        );
        // The same path is otherwise valid; only the protected-folder rule rejects it.
        assert!(
            check_env(
                &env("BRIGADIER_DATA_DIR", via_link.to_str().unwrap()),
                workdir,
                &[]
            )
            .is_ok()
        );
        assert!(check_env(&env("1BAD", "x"), workdir, &protected).is_err());
        assert!(check_env(&env("A B", "x"), workdir, &protected).is_err());
    }

    #[tokio::test]
    async fn stale_preview_data_disposes_cleanly_and_later_records_survive_restart() {
        use crate::ledger::CleanupLedger;
        use crate::manager::flow::{Flow, Options, Reply};
        use brigadier_providers::{claude::Claude, codex::Codex};
        use std::os::unix::fs::MetadataExt;

        let flow = Flow::start(
            "stale-preview-data",
            Options::default(),
            Arc::new(|_| Box::pin(async { Reply::text("") })),
        )
        .await;
        let runtime = &flow.manager.runtime;
        let ledger = runtime.ledger();
        // Preview data is only removed under /tmp, which macOS's temp folder is not.
        let root = crate::manager::workers::test_data_root()
            .join(format!("preview-data-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let artifact = |path: &Path| {
            let meta = std::fs::metadata(path).unwrap();
            Artifact::PreviewDataDir {
                path: path.to_string_lossy().into_owned(),
                identity: Some((meta.dev(), meta.ino())),
            }
        };
        // `None`: the owner held nothing when it was disposed of.
        for replaced in [Some(true), Some(false), None] {
            let case = format!("{replaced:?}");
            let owner = format!("preview-data:{}:{case}", flow.conversation);
            let path = root.join(format!("data-{case}"));
            if let Some(replaced) = replaced {
                std::fs::create_dir(&path).unwrap();
                ledger.record(&owner, artifact(&path)).await.unwrap();
                // Keep the old inode alive so the replacement cannot reuse it.
                std::fs::rename(&path, root.join(format!("original-{case}"))).unwrap();
                if replaced {
                    std::fs::create_dir(&path).unwrap();
                    std::fs::write(path.join("foreign"), "keep").unwrap();
                }
            }
            assert!(ledger.dispose(&owner).await.is_clean());
            assert!(ledger.artifacts(&owner).is_empty());
            assert!(!ledger.disposing().contains(&owner));

            let later = root.join(format!("later-{case}"));
            std::fs::create_dir(&later).unwrap();
            let later_artifact = artifact(&later);
            ledger.record(&owner, later_artifact.clone()).await.unwrap();
            let reloaded = CleanupLedger::load(
                flow.manager.core.clone(),
                runtime.platform().clone(),
                Arc::new(Claude::new(
                    runtime.platform().clone(),
                    runtime.cli_env().clone(),
                )),
                Arc::new(Codex::new(
                    runtime.platform().clone(),
                    runtime.cli_env().clone(),
                )),
            )
            .await
            .unwrap();
            assert!(!reloaded.disposing().contains(&owner));
            reloaded.sweep().await;
            assert!(later.is_dir());
            assert_eq!(reloaded.artifacts(&owner), vec![later_artifact]);
            match replaced {
                Some(true) => assert_eq!(
                    std::fs::read_to_string(path.join("foreign")).unwrap(),
                    "keep"
                ),
                Some(false) => assert!(!path.exists()),
                None => {}
            }
            assert!(ledger.dispose(&owner).await.is_clean());
        }
        std::fs::remove_dir_all(root).unwrap();
        flow.stop().await;
    }

    #[test]
    fn brigadiers_own_repository_needs_a_scratch_data_folder_and_a_dev_identity() {
        let root =
            Temp(std::env::temp_dir().join(format!("brigadier-own-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(root.join("crates/daemon")).unwrap();
        std::fs::create_dir_all(root.join("apps/desktop/src-tauri")).unwrap();
        let none = BTreeMap::new();
        let scratch = BTreeMap::from([("BRIGADIER_DATA_DIR".to_owned(), "/tmp/p".to_owned())]);
        assert!(check_own_repo(&root, &none, "pnpm tauri dev").is_err());
        assert!(check_own_repo(&root, &scratch, "pnpm tauri dev").is_ok());
        assert!(
            check_own_repo(
                &root,
                &scratch,
                "pnpm tauri dev --config '{\"identifier\":\"ai.brigadier.app\"}'"
            )
            .is_err()
        );
        // Any other repository needs neither.
        assert!(check_own_repo(&root.join("crates"), &none, "npm run dev").is_ok());
    }

    #[test]
    fn variables_are_exported_quoted_before_the_command() {
        let env = BTreeMap::from([
            ("PORT".to_owned(), "4000".to_owned()),
            ("TITLE".to_owned(), "it's".to_owned()),
        ]);
        assert_eq!(
            script("npm run dev", &env),
            "export PORT='4000'\nexport TITLE='it'\\''s'\nnpm run dev"
        );
    }

    #[test]
    fn the_tail_keeps_the_last_lines_and_says_how_many_it_left_out() {
        assert_eq!(tail(b"", 5), "");
        assert_eq!(tail(b"a\nb\nc\n", 5), "a\nb\nc");
        assert_eq!(tail(b"a\nb\nc\n", 2), "[\u{2026} 1 earlier lines]\nb\nc");
        let long = "x".repeat(5000);
        let text = format!("{long}\n{long}\n{long}\n");
        let kept = tail(text.as_bytes(), 10);
        assert!(
            kept.starts_with("[\u{2026} 2 earlier lines]"),
            "{}",
            &kept[..40]
        );
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"a\nb\n"), 2);
        assert_eq!(count_lines(b""), 0);
    }

    #[test]
    fn a_long_command_is_named_by_its_start() {
        assert_eq!(short_name("npm run dev\nmore"), "npm run dev");
        let named = short_name(&"y".repeat(60));
        assert_eq!(named.chars().count(), 48);
        assert!(named.ends_with('…'));
    }
}
