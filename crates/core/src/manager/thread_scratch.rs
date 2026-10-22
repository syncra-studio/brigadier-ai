//! A session thread's scratch, swept once its requests are over.
//!
//! The thread's own commands leave things no task owns: a second checkout to compare with the
//! base, its build output and test data, the processes of a test it killed. They go when
//! every request of the session is over (none works or waits) and nothing of its thread runs:
//! no turn, nothing waiting for one, no background run, no overnight run
//! ([`SessionManager::scratch_sweepable`], the one check the sweep and Free up space share).
//!
//! - **Left behind**: what the thread CLI's finished commands left running out of its process
//!   tree ([`brigadier_providers::ProviderSession::left_behind`]): orphans of a killed test,
//!   detached helpers. A stopped one always goes (SIGKILL ends it). A running one that
//!   listens on a TCP port stays, as a server the user may have asked to keep up; so does
//!   anything another owner of the cleanup ledger records (a preview, a background run, the
//!   full checks), and what still runs below the CLI (its MCP servers). One whose tree or
//!   process group holds such a server (an `npm run dev` above its listening node) keeps it:
//!   ending it would end the server too.
//! - **Its temporary folder** (its session folder's `tmp/`, its commands' and checks'
//!   TMPDIR): what works inside it ends (but a listening server), then everything in it goes
//!   (but where a kept server works: the whole folder, when it works in the folder itself);
//!   the folder stays, as its CLI's TMPDIR.
//! - **Its Claude commands' temp folder**: a sandboxed Claude command's TMPDIR is
//!   `<CLAUDE_CODE_TMPDIR>/claude-<uid>`, beside Claude's own files there (a folder per
//!   working directory, named from its path, so starting with `-`; `bash-edit-diff`;
//!   `cache-break-state-…`), which stay. Everything else there goes.
//!
//! A sweep enters a folder only while it is a real folder of this user's, reached without a
//! link at its own name ([`owned_root`]): a link put in its place would aim the sweep at
//! whatever it points to.
//!
//! A sweep reserves the thread first ([`super::conversation::ConvLive::begin_scratch_sweep`]):
//! no turn starts until it is done, and it looks for one then. The launch sweeps every
//! session's before recovery starts anything (nothing of a thread survives a restart).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use brigadier_providers::Artifact;
use brigadier_sandbox::removal;

use super::conversation::ConvLive;
use super::{SessionManager, blocking};
use crate::Result;
use crate::model::{ConversationId, ConversationKind, Lifecycle};
use crate::work::{BackgroundRunState, RequestState};

/// What a sweep of a thread's scratch did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Swept {
    /// The bytes it gave back.
    pub bytes: u64,
    /// The processes it ended.
    pub ended: usize,
    /// What it kept, and why: a listening server, an entry that couldn't go.
    pub kept: Vec<String>,
}

/// One process a sweep looks at: its pid and start time, whether it is stopped, whether it
/// listens on a TCP port, and a listening server its tree or process group holds.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    pid: u32,
    started: Option<f64>,
    stopped: bool,
    listens: bool,
    shelters: Option<u32>,
}

/// Claude's own entries in its commands' temp folder, which a sweep keeps.
fn claude_own(name: &str) -> bool {
    name.starts_with('-') || name == "bash-edit-diff" || name.starts_with("cache-break-state-")
}

/// Whether session `id`'s board says nothing of it is going: every request is over and no
/// background run of its thread runs.
fn board_over(board: &crate::board::Board) -> bool {
    board
        .requests
        .values()
        .all(|request| !matches!(request.state, RequestState::Working | RequestState::Waiting))
        && board
            .background_runs
            .values()
            .all(|run| run.state != BackgroundRunState::Running)
}

impl SessionManager {
    /// The shared check of whether session `id`'s thread scratch may go now: an open session,
    /// not closing, with no overnight run, every request over (none works or waits), no
    /// background run, and nothing of its driver holding the thread. Returns its driver.
    pub(crate) async fn scratch_sweepable(&self, id: &ConversationId) -> Option<Arc<ConvLive>> {
        let conversation = self.core.conversation(id).ok()?;
        if conversation.kind != ConversationKind::Session
            || conversation.deleting
            || conversation.lifecycle == Lifecycle::Archived
            || self.is_closing(id)
            || self.overnight.active.get(id).is_some()
        {
            return None;
        }
        let board = self.core.board(id).await.ok()?;
        if !board_over(&board) {
            return None;
        }
        let conv = self.conv(id).ok()?;
        conv.scratch_idle().await.then_some(conv)
    }

    /// The requests of `conv` settled: once a turn ran since the last sweep and they are all
    /// over, its thread's scratch is swept, in the background.
    pub(super) fn sweep_scratch_when_over(&self, conv: &Arc<ConvLive>) {
        if conv.kind != ConversationKind::Session || !conv.scratch_used.load(Ordering::SeqCst) {
            return;
        }
        let (manager, id) = (self.arc(), conv.id.clone());
        self.spawn(async move {
            match manager.sweep_thread_scratch(&id).await {
                Ok(Some(swept)) if swept.bytes > 0 || swept.ended > 0 || !swept.kept.is_empty() => {
                    tracing::info!(conversation = %id, bytes = swept.bytes, ended = swept.ended, kept = ?swept.kept, "swept the thread's scratch");
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(conversation = %id, error = %err, "could not sweep the thread's scratch");
                }
            }
        });
    }

    /// Sweeps session `id`'s thread scratch (see the module docs), when the shared check
    /// passes; `None` when it doesn't. The thread is reserved meanwhile.
    pub(crate) async fn sweep_thread_scratch(&self, id: &ConversationId) -> Result<Option<Swept>> {
        let Some(conv) = self.scratch_sweepable(id).await else {
            return Ok(None);
        };
        if !conv.begin_scratch_sweep().await {
            return Ok(None);
        }
        // No turn starts now: look again, in case one ran between the check and the hold.
        let swept = match self.core.board(id).await {
            Ok(board) if board_over(&board) && self.overnight.active.get(id).is_none() => {
                conv.scratch_used.store(false, Ordering::SeqCst);
                self.sweep_reserved(id, &conv).await.map(Some)
            }
            Ok(_) => Ok(None),
            Err(err) => Err(err),
        };
        conv.end_scratch_sweep().await;
        self.kick(&conv);
        swept
    }

    /// Sweeps every session's thread scratch at launch, before recovery: no thread runs, so
    /// whatever is there was left by a crash or a quit.
    pub(super) async fn sweep_scratch_at_launch(&self) {
        for conversation in self.core.catalog().conversations {
            if conversation.kind != ConversationKind::Session
                || conversation.deleting
                || conversation.lifecycle == Lifecycle::Archived
            {
                continue;
            }
            let id = conversation.id.clone();
            match self.sweep_scratch_folders(&id, Vec::new()).await {
                Ok(swept) if swept.bytes > 0 || swept.ended > 0 => {
                    tracing::info!(conversation = %id, bytes = swept.bytes, ended = swept.ended, "swept a thread's scratch left from before the restart");
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(conversation = %id, error = %err, "could not sweep a thread's scratch at launch");
                }
            }
        }
    }

    /// The sweep itself, with the thread reserved.
    async fn sweep_reserved(&self, id: &ConversationId, conv: &Arc<ConvLive>) -> Result<Swept> {
        let left = match conv.live_cli().await {
            Some(cli) => cli.session.left_behind(),
            None => Vec::new(),
        };
        self.sweep_scratch_folders(id, left).await
    }

    /// Ends what `left` (the CLI's left-behind processes) and the scratch folders hold, but
    /// what must stay, then empties the folders.
    async fn sweep_scratch_folders(
        &self,
        id: &ConversationId,
        left: Vec<(u32, Option<f64>)>,
    ) -> Result<Swept> {
        let tmp = self.session_dir(id).tmp();
        let command_temps = self.claude_command_temps(id);
        let others = self.recorded_elsewhere(id);
        let platform = self.runtime.platform().clone();
        // Who works in the folders, and what each process is.
        let (tmp_in, folders) = (tmp.clone(), command_temps.clone());
        let looked = blocking(move || {
            let processes = platform.processes();
            let mut pids: Vec<(u32, Option<f64>)> = left;
            for dir in std::iter::once(&tmp_in).chain(&folders) {
                let Some(root) = owned_root(dir) else {
                    continue;
                };
                for pid in processes.in_dir(&root).unwrap_or_default() {
                    if !pids.iter().any(|(seen, _)| *seen == pid) {
                        pids.push((pid, processes.start_time_ms(pid).ok()));
                    }
                }
            }
            Ok(pids
                .into_iter()
                .filter(|(pid, _)| !others.contains(pid))
                .map(|(pid, started)| {
                    let below: Vec<u32> = processes
                        .descendants(pid)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|&child| !processes.is_stopped(child))
                        .collect();
                    let candidate = Candidate {
                        pid,
                        started,
                        stopped: processes.is_stopped(pid),
                        listens: false,
                        shelters: None,
                    };
                    (candidate, below)
                })
                .collect::<Vec<_>>())
        })
        .await?;
        // What listens on a port: the candidates that run, and what runs below them. One that
        // can't be checked counts as listening, so a missing `lsof` spares a server.
        let mut asked = HashSet::new();
        let mut listeners = Vec::new();
        for (candidate, below) in &looked {
            let running = (!candidate.stopped).then_some(candidate.pid);
            for pid in running.into_iter().chain(below.iter().copied()) {
                if asked.insert(pid)
                    && super::preview_ready::process_listening_ports(pid)
                        .await
                        .is_none_or(|ports| !ports.is_empty())
                {
                    listeners.push(pid);
                }
            }
        }
        let platform = self.runtime.platform().clone();
        let processes = platform.processes();
        let groups: Vec<(u32, Option<u32>)> = listeners
            .iter()
            .map(|&pid| (pid, processes.group_of(pid)))
            .collect();
        let candidates: Vec<Candidate> = looked
            .into_iter()
            .map(|(mut candidate, below)| {
                candidate.listens = listeners.contains(&candidate.pid);
                // Ending a candidate ends its tree and the process groups its members lead.
                let in_tree = |pid: u32| pid == candidate.pid || below.contains(&pid);
                candidate.shelters = groups
                    .iter()
                    .find(|(server, group)| {
                        *server != candidate.pid
                            && (below.contains(server) || group.is_some_and(in_tree))
                    })
                    .map(|(server, _)| *server);
                candidate
            })
            .collect();
        blocking(move || {
            Ok(sweep_blocking(
                &*platform,
                &candidates,
                &tmp,
                &command_temps,
            ))
        })
        .await
    }

    /// The command temp folders of the Claude CLIs session `id`'s thread ran, as the cleanup
    /// ledger records them ([`command_temps`]).
    fn claude_command_temps(&self, id: &ConversationId) -> Vec<PathBuf> {
        let Ok(conversation) = self.core.conversation(id) else {
            return Vec::new();
        };
        let (owner, _) = super::lifecycle::conversation_owner(&conversation);
        command_temps(&self.runtime.ledger().artifacts(&owner))
    }

    /// The processes other owners than session `id`'s thread record in the cleanup ledger
    /// (previews, background runs, checks, workers): a sweep never ends them.
    fn recorded_elsewhere(&self, id: &ConversationId) -> HashSet<u32> {
        let own = self
            .core
            .conversation(id)
            .map(|conversation| super::lifecycle::conversation_owner(&conversation).0)
            .unwrap_or_default();
        self.runtime
            .ledger()
            .owners()
            .into_iter()
            .filter(|(owner, _, _)| *owner != own)
            .flat_map(|(_, artifacts, _)| artifacts)
            .filter_map(|artifact| match artifact {
                Artifact::Process { pid, .. } => Some(pid),
                _ => None,
            })
            .collect()
    }
}

/// The command temp folders (`<CLAUDE_CODE_TMPDIR>/claude-<uid>`) of the Claude CLIs whose
/// recorded `artifacts` name them.
pub(crate) fn command_temps(artifacts: &[Artifact]) -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let uid = nix::unistd::getuid();
        artifacts
            .iter()
            .filter_map(|artifact| match artifact {
                Artifact::ClaudeTempDir { path } => {
                    Some(Path::new(path).join(format!("claude-{uid}")))
                }
                _ => None,
            })
            .collect()
    }
    #[cfg(not(unix))]
    {
        let _ = artifacts;
        Vec::new()
    }
}

/// What a sweep of the thread temp folder `tmp` and the Claude command temp folders would
/// remove: their entries but Claude's own.
pub(crate) fn leftovers(tmp: &Path, command_temps: &[PathBuf]) -> Vec<PathBuf> {
    let entries = |dir: &Path, keep: fn(&str) -> bool| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| !keep(&entry.file_name().to_string_lossy()))
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut found = owned_root(tmp)
        .map(|root| entries(&root, |_| false))
        .unwrap_or_default();
    for root in command_temps.iter().filter_map(|dir| owned_root(dir)) {
        found.extend(entries(&root, claude_own));
    }
    found
}

/// `dir` as a sweep may enter it: a real folder (not a link) of this user's, its parent's
/// links resolved (`/tmp` is one on macOS). `None` for anything else, which a sweep leaves
/// alone: a link put in its place would aim the sweep at whatever it points to.
pub(crate) fn owned_root(dir: &Path) -> Option<PathBuf> {
    let meta = std::fs::symlink_metadata(dir).ok()?;
    if !meta.file_type().is_dir() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if meta.uid() != nix::unistd::getuid().as_raw() {
            return None;
        }
    }
    let root = dir.parent()?.canonicalize().ok()?.join(dir.file_name()?);
    (root.canonicalize().ok()? == root).then_some(root)
}

/// Ends the candidates that may go (stopped, or not listening), then empties `tmp` and the
/// Claude command temp folders, keeping Claude's own entries and any entry a kept process
/// works in.
fn sweep_blocking(
    platform: &dyn brigadier_sandbox::Platform,
    candidates: &[Candidate],
    tmp: &Path,
    command_temps: &[PathBuf],
) -> Swept {
    let processes = platform.processes();
    let mut swept = Swept::default();
    let mut kept_dirs: Vec<PathBuf> = Vec::new();
    // A kept process whose folder can't be told (no `lsof`) may work in any of them.
    let mut kept_somewhere = false;
    for candidate in candidates {
        let same = processes.is_alive(candidate.pid)
            && candidate.started.is_some_and(|started| {
                processes
                    .start_time_ms(candidate.pid)
                    .is_ok_and(|now| (now - started).abs() < 1_000.0)
            });
        if !same {
            continue;
        }
        if let Some(server) = candidate.shelters {
            // Ending its tree would end the server: a stopped one is ended alone (its
            // SIGTERM lands once it goes on), a running one stays.
            if candidate.stopped {
                match processes
                    .terminate(candidate.pid)
                    .and_then(|()| processes.resume(candidate.pid))
                {
                    Ok(()) => swept.ended += 1,
                    Err(err) => swept.kept.push(format!("process {}: {err}", candidate.pid)),
                }
            } else {
                swept.kept.push(format!(
                    "process {}: process {server} below it listens on a port",
                    candidate.pid
                ));
            }
            match working_dir(server) {
                Some(cwd) => kept_dirs.push(cwd),
                None => kept_somewhere = true,
            }
            continue;
        }
        if candidate.listens && !candidate.stopped {
            let what = processes
                .command_line(candidate.pid)
                .map(|words| words.join(" "))
                .unwrap_or_default();
            swept.kept.push(format!(
                "process {} listens on a port: {what}",
                candidate.pid
            ));
            match working_dir(candidate.pid) {
                Some(cwd) => kept_dirs.push(cwd),
                None => kept_somewhere = true,
            }
            continue;
        }
        match processes.kill_tree(candidate.pid) {
            Ok(()) => swept.ended += 1,
            Err(err) => swept.kept.push(format!("process {}: {err}", candidate.pid)),
        }
    }
    if kept_somewhere {
        swept
            .kept
            .push("the folders: a kept process works in one, and which can't be told".to_owned());
        return swept;
    }
    empty(tmp, |_| false, &kept_dirs, &mut swept);
    for dir in command_temps {
        empty(dir, claude_own, &kept_dirs, &mut swept);
    }
    swept
}

/// Removes every entry of `dir` but those `keep` names and those a kept process works in, or
/// none when a kept process works in `dir` itself. Only a folder [`owned_root`] passes.
fn empty(dir: &Path, keep: impl Fn(&str) -> bool, kept_dirs: &[PathBuf], swept: &mut Swept) {
    let Some(root) = owned_root(dir) else {
        if std::fs::symlink_metadata(dir).is_ok() {
            swept.kept.push(format!(
                "{}: not a folder of this user's own",
                dir.display()
            ));
        }
        return;
    };
    if kept_dirs.contains(&root) {
        swept
            .kept
            .push(format!("{}: a kept process works there", root.display()));
        return;
    }
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if keep(&name) {
            continue;
        }
        let path = root.join(&name);
        if kept_dirs.iter().any(|cwd| cwd.starts_with(&path)) {
            swept
                .kept
                .push(format!("{}: a kept process works there", path.display()));
            continue;
        }
        let bytes = removal::allocated_size(&path);
        // The root is bound again: a link put at its name since is refused.
        let bound = removal::bind(&root, &path).and_then(|bound| {
            if bound.root() == root {
                Ok(bound)
            } else {
                Err(removal::RemovalError::Symlink(root.clone()))
            }
        });
        match bound.and_then(|bound| removal::delete(&bound)) {
            Ok(()) => swept.bytes += bytes,
            Err(err) => swept.kept.push(format!("{}: {err}", path.display())),
        }
    }
}

/// The working directory of `pid`, where it can be told.
fn working_dir(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("/usr/sbin/lsof")
            .args(["-a", "-d", "cwd", "-Fn", "-p", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix('n'))
            .map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = pid;
        None
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A kept server whose folder can't be told (no `lsof` to ask) may work in any of the
    /// folders, so none is emptied.
    #[test]
    fn a_kept_process_in_an_unknown_folder_keeps_every_folder() {
        let dir = std::env::temp_dir().join(format!("brigadier-sweep-{}", uuid::Uuid::now_v7()));
        let tmp = dir.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("asset.txt"), "kept").unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.join("data")),
        })
        .unwrap();
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id();
        let candidate = Candidate {
            pid,
            started: platform.processes().start_time_ms(pid).ok(),
            stopped: false,
            listens: false,
            // A server below it that has exited since: its folder can't be read.
            shelters: Some(u32::MAX - 1),
        };
        let swept = sweep_blocking(&*platform, &[candidate], &tmp, &[]);
        let _ = child.kill();
        let _ = child.wait();
        let left = tmp.join("asset.txt").exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(left, "{swept:?}");
        assert!(
            swept
                .kept
                .iter()
                .any(|kept| kept.starts_with("the folders"))
        );
    }
}
