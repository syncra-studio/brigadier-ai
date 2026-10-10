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
//! - It stops on `stop_preview`, the user's Stop, the merge of the session branch, archive and
//!   delete (before the `session:<id>` owner's worktree goes) and Brigadier's quit; the launch
//!   sweep ends any a crash left running, and its record reads as stopped.
//! - Stopping it is SIGTERM to its process group, then, after [`STOP_GRACE`], SIGKILL to it
//!   and everything it started.
//! - Its stdout and stderr go to one log file in `<data>/previews/<conversation>`. Reading it
//!   (`preview_log`) stores a snapshot in the blob store under an `out-<id>` alias, as a long
//!   command output is (`super::tool_output`), and the final log is stored when it ends.
//!
//! **Access.** The daemon starts it, so the CLI's sandbox doesn't hold it; it gets the
//! thread's access instead. At Full access it runs as it is. On macOS, other levels use
//! Brigadier's GUI Seatbelt profile even when Codex is installed: the same writable roots,
//! denied reads, network and Unix socket grants, plus window surfaces and Chromium helper
//! rendezvous for dev/test identities. A private temp directory and a newly created,
//! validated app data directory are writable.
//! Chromium/Electron may need `--no-sandbox` to avoid nesting their sandbox; Brigadier's
//! outer confinement still applies to every helper. Linux keeps the command sandbox path.
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

use brigadier_providers::{Access, Artifact, ProviderKind};
use tokio_util::sync::CancellationToken;

use super::{OUTPUT_MAX_BYTES, SessionManager, blocking};
use crate::model::{ConversationId, DomainEvent, Lifecycle};
use crate::tools::StartPreview;
use crate::work::{OutputSource, Preview, PreviewState};
use crate::{Error, Result, now_ms};

/// How long a stopped preview has to exit after SIGTERM before it is killed.
/// Why a preview the user stopped ended: their Stop on its chip, or on the conversation.
pub(crate) const USER_STOP: &str = "stopped by the user";
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long `start_preview` watches a new preview for an early exit before it answers.
const START_WATCH: Duration = Duration::from_millis(1500);
/// Lines `preview_log` shows by default, and at most.
const TAIL_DEFAULT: u32 = 40;
const TAIL_MAX: u32 = 400;
/// Lines of output `start_preview` shows.
const START_TAIL: u32 = 20;
/// Most bytes of a log's end a reply shows.
const TAIL_BYTES_MAX: usize = 8 * 1024;
/// The data-folder area of the previews' logs.
const LOG_AREA: &str = "previews";

/// The cleanup-ledger owner of a conversation's previews.
pub(crate) fn preview_owner(id: &ConversationId) -> String {
    format!("preview:{id}")
}

/// A running preview.
pub(crate) struct LivePreview {
    conversation: ConversationId,
    id: String,
    pid: u32,
    workspace: PathBuf,
    log: PathBuf,
    /// Why it is being stopped (the first reason given wins).
    reason: Mutex<Option<String>>,
    /// Asks its watcher to stop it.
    stop: CancellationToken,
    /// Cancelled once its end is recorded.
    ended: CancellationToken,
    /// The log's size and alias at its last snapshot, so an unchanged log isn't stored again.
    snapshot: Mutex<Option<(u64, String)>>,
    /// Held while its record is read and written back, so a log snapshot can't undo its end.
    updating: tokio::sync::Mutex<()>,
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
        if cfg!(windows) {
            return Err(Error::Invalid(
                "previews run on macOS and Linux only".into(),
            ));
        }
        self.admit()?;
        let stops = self.previews.stops_of(id);
        let command = args.command.trim().to_owned();
        if command.is_empty() {
            return Err(Error::Invalid("`command` is empty".into()));
        }
        let conversation = self.core.conversation(id)?;
        if conversation.lifecycle == Lifecycle::Archived || conversation.deleting {
            return Err(Error::Invalid("this session is closed".into()));
        }
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
        let workdir = preview_workdir(args.workdir.as_deref(), &workspace)?;
        let mut env = args.env.unwrap_or_default();
        let installed = brigadier_sandbox::default_data_dir().ok();
        let protected = [Some(self.data_dir.clone()), installed]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        check_env(&env, &workdir, &protected)?;
        check_own_repo(&workspace, &env, &command)?;
        let scratch = self.owned_dir("orch", &id.0);
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
        let number = self.core.board(id).await?.previews.len() + 1;
        let preview_id = format!("preview-{number}");
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
        #[cfg(unix)]
        let mut data = if let Some(value) = env.get(brigadier_sandbox::DATA_DIR_ENV) {
            let owner = format!("session:{id}");
            let data =
                create_preview_data(&workdir.join(value), &protected, &ledger.artifacts(&owner))?;
            ledger
                .record(
                    &owner,
                    Artifact::ScratchDir {
                        path: data.path.to_string_lossy().into_owned(),
                    },
                )
                .await?;
            env.insert(
                brigadier_sandbox::DATA_DIR_ENV.into(),
                data.path.to_string_lossy().into_owned(),
            );
            Some(data)
        } else {
            None
        };
        #[cfg(target_os = "macos")]
        let (env, writable_roots, preview_temp) = {
            let mut env = env;
            let mut roots = Vec::new();
            let mut preview_temp = None;
            if launch.access != Access::Full {
                let temp = self.preview_temp(&owner).await?;
                for name in ["TMPDIR", "MAC_CHROMIUM_TMPDIR"] {
                    env.entry(name.into())
                        .or_insert_with(|| temp.to_string_lossy().into_owned());
                }
                preview_temp = Some(temp);
                if let Some(data) = &data {
                    roots.push(data.path.clone());
                }
            }
            (env, roots, preview_temp)
        };
        #[cfg(not(target_os = "macos"))]
        let (writable_roots, preview_temp): (Vec<PathBuf>, Option<PathBuf>) = (Vec::new(), None);
        let mut spec = self.preview_spec(
            &launch.access,
            &workdir,
            &script(&command, &env),
            &writable_roots,
            preview_temp.as_deref(),
        )?;
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
            state: PreviewState::Running,
            started_at_ms: now_ms(),
            ended_at_ms: None,
            log: None,
        };
        let live = Arc::new(LivePreview {
            conversation: id.clone(),
            id: preview_id.clone(),
            pid,
            workspace,
            log,
            reason: Mutex::default(),
            stop: CancellationToken::new(),
            ended: CancellationToken::new(),
            snapshot: Mutex::default(),
            updating: tokio::sync::Mutex::default(),
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
            let recorded = this.record_preview(preview).await;
            if recorded.is_err() {
                watched.ask_to_stop("it could not be recorded");
            }
            drop(starting);
            #[cfg(unix)]
            let recorded_ok = recorded.is_ok();
            let _ = recorded_tx.send(recorded);
            #[cfg(unix)]
            if recorded_ok && let Some(data) = &mut data {
                data.created = false;
            }
            this.watch_preview(watched, child, artifact).await;
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
        if tokio::time::timeout(START_WATCH, live.ended.cancelled())
            .await
            .is_ok()
        {
            return self
                .preview_log(id, Some(&preview_id), Some(START_TAIL))
                .await
                .map(|log| format!("{preview_id} ended at once.\n{log}"));
        }
        let output = self
            .preview_log(id, Some(&preview_id), Some(START_TAIL))
            .await
            .unwrap_or_default();
        Ok(format!(
            "{preview_id} \"{name}\" is running (pid {pid}, its own process group) in {}. It keeps \
             running across your turns until you call stop_preview, and stops when the session \
             is merged, archived or deleted, when your workspace changes, when the user presses \
             Stop and when Brigadier quits. Its output so far:\n{output}",
            workdir.display()
        ))
    }

    /// A short, private folder for Chromium's Unix socket (long worktree paths overflow
    /// sun_path). Track it before creating it so cancellation and launch failures are swept.
    #[cfg(target_os = "macos")]
    async fn preview_temp(&self, owner: &str) -> Result<PathBuf> {
        #[cfg(not(test))]
        let root = PathBuf::from("/tmp");
        // Tests keep all their writes beneath their isolated TMPDIR.
        #[cfg(test)]
        let root = std::env::temp_dir();
        let dir = root.join(format!(
            "brigadier-pv-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..16]
        ));
        self.runtime
            .ledger()
            .record(
                owner,
                Artifact::ScratchDir {
                    path: dir.to_string_lossy().into_owned(),
                },
            )
            .await?;
        self.runtime
            .platform()
            .private_fs()
            .create_private_dir(&dir)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        Ok(dir)
    }

    /// Full access uses run_spec. Other macOS previews always use GUI Seatbelt, independently
    /// of Codex's installation. Elsewhere, retain the existing command sandbox selection.
    fn preview_spec(
        &self,
        access: &Access,
        workdir: &Path,
        script: &str,
        writable_roots: &[PathBuf],
        preview_temp: Option<&Path>,
    ) -> Result<brigadier_sandbox::SpawnSpec> {
        if *access == Access::Full {
            return self.run_spec(access, workdir, script);
        }
        if cfg!(target_os = "macos") {
            let temp = preview_temp.ok_or_else(|| {
                Error::Invalid("a preview needs its private temp directory".into())
            })?;
            let mut spec = self.runtime.cli_env().spec(Path::new(super::run::SHELL));
            spec.args = vec!["-c".into(), script.into()];
            spec.cwd = Some(workdir.to_owned());
            let mut policy = super::run::seatbelt_policy(access, workdir);
            policy.writable_roots.extend_from_slice(writable_roots);
            policy.writable_roots.push(temp.to_owned());
            let unix_sockets = match access {
                Access::Scoped { unix_sockets, .. } => unix_sockets.as_slice(),
                _ => &[],
            };
            return self
                .runtime
                .platform()
                .sandbox()
                .confine_preview(spec, &policy, unix_sockets, temp)
                .map_err(|err| {
                    Error::Invalid(format!("the preview can't be sandboxed here: {err}"))
                });
        }
        if self
            .runtime
            .cli_env()
            .resolve(ProviderKind::Codex)
            .is_some()
        {
            return self.run_spec(access, workdir, script);
        }
        self.seatbelt_spec(access, workdir, &["-c".into(), script.into()])
            .map_err(|err| Error::Invalid(format!("the preview can't be sandboxed here: {err}")))
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
                terminate_group(&*platform, live.pid);
                if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_err() {
                    // It didn't go: it and everything it started are killed.
                    let _ = platform.processes().kill_tree(live.pid);
                }
                let _ = child.wait().await;
                PreviewState::Stopped { reason: live.reason() }
            }
        };
        // Reaped: what it left in its group goes too.
        let _ = platform.processes().kill_group(live.pid);
        self.runtime
            .ledger()
            .forget(&preview_owner(&live.conversation), artifact)
            .await;
        self.end_preview(&live, state).await;
        self.previews
            .lock()
            .remove(&(live.conversation.clone(), live.id.clone()));
        live.ended.cancel();
    }

    /// Records how a preview ended, with its whole log stored, and removes its log file.
    async fn end_preview(&self, live: &LivePreview, state: PreviewState) {
        let id = &live.conversation;
        tracing::info!(conversation = %id, preview = %live.id, state = ?state, "preview ended");
        let status = match &state {
            PreviewState::Exited { status, .. } => status.clone(),
            PreviewState::Stopped { reason } => format!("stopped: {reason}"),
            PreviewState::Running => "running".into(),
        };
        let log = self.store_log(live, &status).await;
        let _updating = live.updating.lock().await;
        let preview = match self.core.board(id).await {
            Ok(board) => board.previews.get(&live.id).cloned(),
            Err(_) => None,
        };
        if let Some(mut preview) = preview {
            // What the thread didn't do itself and no other note tells it: the user's Stop
            // (the chip's or the conversation's), or an end on its own. It hears of it with
            // the user's next message, so it never says a stopped preview "is running".
            let told = match &state {
                PreviewState::Stopped { reason } if reason == USER_STOP => {
                    Some("was stopped by the user".to_owned())
                }
                PreviewState::Exited { status, .. } => Some(format!("ended on its own ({status})")),
                _ => None,
            };
            if let (Some(told), Ok(conv)) = (told, self.conv(id)) {
                conv.note(format!(
                    "[preview] {} \"{}\" {told}; it is not running now. Start it again if the user wants it.",
                    preview.id, preview.name
                ))
                .await;
            }
            preview.state = state;
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
        if !current.state.is_running() || current.log.as_deref() == Some(alias) {
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

    /// `preview_log`: the end of a preview's output (the latest preview's by default), with an
    /// `out-<id>` reference to its whole log as it stands now.
    pub(crate) async fn preview_log(
        &self,
        id: &ConversationId,
        preview: Option<&str>,
        tail_lines: Option<u32>,
    ) -> Result<String> {
        let board = self.core.board(id).await?;
        let found = match preview.map(str::trim).filter(|wanted| !wanted.is_empty()) {
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

    /// `stop_preview`: stops one preview, or every running one.
    pub(crate) async fn stop_preview_tool(
        &self,
        id: &ConversationId,
        preview: Option<&str>,
    ) -> Result<String> {
        let stopped = self
            .stop_previews_of(id, preview, "stopped by the thread")
            .await?;
        if stopped.is_empty() {
            return Ok("No preview is running.".into());
        }
        Ok(stopped
            .iter()
            .map(|preview| {
                format!(
                    "{} \"{}\": {}",
                    preview.id,
                    preview.name,
                    state_words(preview)
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// The user's Stop on a preview chip: stops `preview`, or every running preview of the
    /// conversation.
    pub async fn stop_preview(&self, id: ConversationId, preview: Option<String>) -> Result<()> {
        self.stop_previews_of(&id, preview.as_deref(), USER_STOP)
            .await
            .map(drop)
    }

    /// Stops `preview` (every running one when `None`) of conversation `id` for `reason`, and
    /// returns them as recorded once ended.
    async fn stop_previews_of(
        &self,
        id: &ConversationId,
        preview: Option<&str>,
        reason: &str,
    ) -> Result<Vec<Preview>> {
        let live = match preview.map(str::trim).filter(|wanted| !wanted.is_empty()) {
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
            None => self.previews.of(id),
        };
        stop_all(&live, reason).await;
        let board = self.core.board(id).await?;
        Ok(live
            .iter()
            .filter_map(|live| board.previews.get(&live.id).cloned())
            .collect())
    }

    /// Stops every running preview of conversation `id` (the user's Stop, a merge, the
    /// session closing).
    pub(crate) async fn stop_previews(&self, id: &ConversationId, reason: &str) {
        // A start already under way is refused, and one that got past that check is waited
        // for, so this stop sees every preview there will be.
        self.previews.count_stop(id);
        drop(self.previews.starting.lock().await);
        stop_all(&self.previews.of(id), reason).await;
    }

    /// Stops the previews of conversation `id` that run in another workspace than the
    /// thread's now (an overnight run started or ended).
    pub(crate) async fn stop_moved_previews(&self, id: &ConversationId) {
        let running = self.previews.of(id);
        if running.is_empty() {
            return;
        }
        let now = self.recorded_workspace(id).map(|workspace| workspace.path);
        let moved: Vec<_> = running
            .into_iter()
            .filter(|live| Some(&live.workspace) != now.as_ref())
            .collect();
        stop_all(&moved, "the thread's workspace changed").await;
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
            .filter(|preview| preview.state.is_running())
        {
            let live = LivePreview {
                conversation: id.clone(),
                id: preview.id.clone(),
                pid: preview.pid.unwrap_or_default(),
                workspace: PathBuf::from(&preview.workspace),
                log: dir.join(format!("{}.log", preview.id)),
                reason: Mutex::new(Some("Brigadier quit".into())),
                stop: CancellationToken::new(),
                ended: CancellationToken::new(),
                snapshot: Mutex::default(),
                updating: tokio::sync::Mutex::default(),
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

/// Asks each of `live` to stop for `reason`, and waits until each one's end is recorded.
async fn stop_all(live: &[Arc<LivePreview>], reason: &str) {
    for preview in live {
        preview.ask_to_stop(reason);
    }
    for preview in live {
        preview.ended.cancelled().await;
    }
}

/// SIGTERM to the process group `pid` leads (still the caller's child: not yet reaped).
fn terminate_group(platform: &dyn brigadier_sandbox::Platform, pid: u32) {
    #[cfg(unix)]
    {
        let _ = platform;
        if let Ok(pid) = i32::try_from(pid) {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            );
        }
    }
    #[cfg(not(unix))]
    {
        let _ = platform.processes().terminate(pid);
    }
}

/// The folder a preview runs in: the workspace, or `requested` (relative to it), which must be
/// inside it.
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

/// A newly claimed directory rolls back on failure or cancellation. A reused session
/// directory survives failed starts, just as it survives a successful preview's exit.
#[cfg(unix)]
struct PreviewData {
    path: PathBuf,
    parent: std::os::fd::OwnedFd,
    name: std::ffi::OsString,
    created: bool,
    bound: Option<brigadier_sandbox::removal::Bound>,
}

#[cfg(unix)]
impl Drop for PreviewData {
    fn drop(&mut self) {
        if self.created {
            if let Some(bound) = &self.bound {
                if let Err(err) = brigadier_sandbox::removal::delete(bound) {
                    tracing::warn!(path = %self.path.display(), error = %err, "could not roll back preview data; the session ledger will retry");
                }
                return;
            }
            // Before binding the new leaf, it is empty. Never follow a replaced parent.
            let _ = nix::unistd::unlinkat(
                &self.parent,
                self.name.as_os_str(),
                nix::unistd::UnlinkatFlags::RemoveDir,
            );
        }
    }
}

#[cfg(unix)]
fn directory_path(fd: &std::os::fd::OwnedFd) -> std::io::Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut path = PathBuf::new();
        nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETPATH(&mut path))?;
        Ok(path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        use std::os::fd::AsRawFd as _;
        std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
    }
}

/// Walk from the temp root without following links, then claim the leaf through its
/// parent's descriptor. Only this session's ledger grants permission to reuse a leaf.
#[cfg(unix)]
fn create_preview_data(
    path: &Path,
    protected: &[PathBuf],
    owned: &[Artifact],
) -> Result<PreviewData> {
    use nix::fcntl::{OFlag, open, openat};
    use nix::sys::stat::{Mode, mkdirat};
    use std::path::Component;

    let invalid = |why: String| {
        Error::Invalid(format!(
            "BRIGADIER_DATA_DIR needs an existing, non-symlink parent under /tmp \
         (only this session's recorded folder may be reused); pick a fresh name: {why}"
        ))
    };
    let check = |dir: &Path| {
        check_env(
            &BTreeMap::from([(
                brigadier_sandbox::DATA_DIR_ENV.into(),
                dir.to_string_lossy().into_owned(),
            )]),
            Path::new("/"),
            protected,
        )
    };
    check(path)?;
    let temp = Path::new("/tmp")
        .canonicalize()
        .map_err(|err| invalid(err.to_string()))?;
    let relative = path
        .strip_prefix("/tmp")
        .or_else(|_| path.strip_prefix(&temp))
        .map_err(|err| invalid(err.to_string()))?;
    let mut parts = relative.components().collect::<Vec<_>>();
    let Some(Component::Normal(name)) = parts.pop() else {
        return Err(invalid(path.display().to_string()));
    };
    if name.to_string_lossy().starts_with("brigadier-pv-")
        || name.to_string_lossy().starts_with("brigadier-test-")
    {
        return Err(invalid("reserved folder name".into()));
    }
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut parent = open(&temp, flags, Mode::empty()).map_err(|err| invalid(err.to_string()))?;
    for part in parts {
        let Component::Normal(part) = part else {
            return Err(invalid("invalid parent component".into()));
        };
        parent =
            openat(&parent, part, flags, Mode::empty()).map_err(|err| invalid(err.to_string()))?;
    }
    let dir = directory_path(&parent)
        .map_err(|err| invalid(err.to_string()))?
        .join(name);
    check(&dir)?;
    let artifact = Artifact::ScratchDir {
        path: dir.to_string_lossy().into_owned(),
    };
    let created = match mkdirat(&parent, name, Mode::S_IRWXU) {
        Ok(()) => true,
        Err(nix::errno::Errno::EEXIST) if owned.contains(&artifact) => false,
        Err(err) => return Err(invalid(err.to_string())),
    };
    let mut data = PreviewData {
        path: dir,
        parent,
        name: name.to_owned(),
        created,
        bound: None,
    };
    let leaf =
        openat(&data.parent, name, flags, Mode::empty()).map_err(|err| invalid(err.to_string()))?;
    let real = directory_path(&leaf).map_err(|err| invalid(err.to_string()))?;
    check(&real)?;
    if real != data.path || data.path.canonicalize().ok().as_ref() != Some(&real) {
        return Err(invalid("parent changed while creating the folder".into()));
    }
    data.bound = Some(
        brigadier_sandbox::removal::bind(&temp, &data.path)
            .map_err(|err| invalid(err.to_string()))?,
    );
    Ok(data)
}

/// `path` as the file system resolves it (`/tmp` → `/private/tmp`), down to the part of it
/// that exists; the rest, which doesn't exist yet, as written (`..` and `.` taken out).
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

fn state_words(preview: &Preview) -> String {
    match &preview.state {
        PreviewState::Running => match preview.pid {
            Some(pid) => format!("running, pid {pid}"),
            None => "running".into(),
        },
        PreviewState::Exited { status, .. } => format!("ended on its own ({status})"),
        PreviewState::Stopped { reason } => format!("stopped ({reason})"),
    }
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
    use super::*;

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

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_preview_uses_gui_seatbelt_and_tracks_its_private_temp() {
        use crate::manager::flow::{Flow, Options, Reply};

        let flow = Flow::start(
            "preview-profile",
            Options::default(),
            Arc::new(|_| Box::pin(async { Reply::text("[quiet]") })),
        )
        .await;
        let manager = &flow.manager;
        // This must remain Seatbelt when the login environment resolves Codex, too.
        let owner = preview_owner(&flow.conversation);
        let temp = manager.preview_temp(&owner).await.unwrap();
        assert!(temp.is_dir());
        assert!(
            manager
                .runtime
                .ledger()
                .artifacts(&owner)
                .contains(&Artifact::ScratchDir {
                    path: temp.to_string_lossy().into_owned(),
                })
        );
        let data = temp.join("new-app-data");
        let extra = [data.clone()];
        let command = format!(
            "mkdir {}; touch {}/probe",
            shell_quote(data.to_str().unwrap()),
            shell_quote(temp.to_str().unwrap())
        );
        for access in [
            Access::Workspace {
                extra_roots: Vec::new(),
            },
            Access::Scoped {
                write_cwd: false,
                writable_roots: vec![flow.repo.clone()],
                network: false,
                deny_read: vec![flow.repo.join("secret")],
                unix_sockets: vec![flow.repo.join("granted.sock")],
            },
        ] {
            let spec = manager
                .preview_spec(&access, &flow.repo, &command, &extra, Some(&temp))
                .unwrap();
            assert_eq!(spec.program, Path::new("/usr/bin/sandbox-exec"));
            let profile = spec.args[1].to_str().unwrap();
            assert!(profile.contains("IOSurfaceRootUserClient"));
            assert!(profile.contains("MachPortRendezvousServer"));
            if matches!(access, Access::Scoped { .. }) {
                assert!(profile.contains("DENY_READ_0"));
                assert!(profile.contains("UNIX_SOCKET_0"));
                assert!(!profile.contains("(allow network-outbound)"));
            }
            let out = manager
                .runtime
                .platform()
                .processes()
                .piped_command(&spec)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
            std::fs::remove_dir(&data).unwrap();
        }
        let full = manager
            .preview_spec(&Access::Full, &flow.repo, "true", &extra, None)
            .unwrap();
        let run = manager.run_spec(&Access::Full, &flow.repo, "true").unwrap();
        assert_eq!(full.program, run.program);
        assert_eq!(full.args, run.args);
        assert!(manager.runtime.ledger().dispose(&owner).await.is_clean());
        assert!(!temp.exists());
        flow.stop().await;
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
        #[cfg(target_os = "macos")]
        {
            let fresh = root.join("fresh-data");
            let mut data = create_preview_data(&fresh, &[], &[]).unwrap();
            assert_eq!(data.path, resolved(&fresh));
            data.created = false;
            assert!(create_preview_data(&fresh, &[], &[]).is_err());
            assert!(create_preview_data(&root.join("link"), &[], &[]).is_err());
            for name in ["brigadier-pv-new", "brigadier-test-new"] {
                assert!(create_preview_data(&root.join(name), &[], &[]).is_err());
                assert!(!root.join(name).exists());
            }
        }
        assert!(check_env(&env("1BAD", "x"), workdir, &protected).is_err());
        assert!(check_env(&env("A B", "x"), workdir, &protected).is_err());
    }

    #[test]
    fn preview_data_refuses_symlink_parents_and_rechecks_protected_paths() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root =
            Temp(std::env::temp_dir().join(format!("preview-data-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(root.join("parent")).unwrap();
        std::fs::create_dir(root.join("protected")).unwrap();
        let path = root.join("parent/data");
        let env = BTreeMap::from([(
            brigadier_sandbox::DATA_DIR_ENV.into(),
            path.to_string_lossy().into_owned(),
        )]);
        let protected = [root.join("protected")];
        check_env(&env, Path::new("/"), &protected).unwrap();
        // Swap after the early validation, as a thread could while record().await runs.
        std::fs::remove_dir(root.join("parent")).unwrap();
        symlink(root.join("protected"), root.join("parent")).unwrap();
        assert!(create_preview_data(&path, &protected, &[]).is_err());
        assert!(!root.join("protected/data").exists());
        // Even a symlink pointing to an otherwise allowed parent is refused.
        assert!(create_preview_data(&path, &[], &[]).is_err());
        assert!(create_preview_data(&root.join("protected/data"), &protected, &[]).is_err());
        let fresh = root.join("fresh");
        let data = create_preview_data(&fresh, &protected, &[]).unwrap();
        assert_eq!(
            std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::write(
            fresh.join("partial-start"),
            "a child wrote before recording failed",
        )
        .unwrap();
        drop(data);
        assert!(
            !fresh.exists(),
            "a cancelled start rolls back its new directory"
        );
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
