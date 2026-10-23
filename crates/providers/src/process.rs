//! A CLI child process Brigadier talks to over stdio.
//!
//! The child leads its own process group. Four tasks serve it: stdout is split into lines and
//! handed to the adapter, stderr keeps a short tail for error reports, a tracker walks its
//! process tree every second while it runs, and a waiter reaps the child, then ends whatever
//! it left behind.
//!
//! The tracker is what makes "nothing survives" hold. Both CLIs start tool commands in
//! sessions of their own (`setsid`), and a command can put a server in the background and
//! exit, leaving the server parented to `launchd`/`init` and out of the CLI's tree. Every
//! process seen in the tree is remembered with its start time and ended when the CLI goes,
//! however it goes, and each group or session leader among them is also recorded in the
//! cleanup ledger, so the crash sweep finds it after the daemon died.
//!
//! A session's or a Chat's CLI ([`Options::keeps_detached`]) is the exception: what its commands
//! leave running outlives it, as with the plain CLI (STRIP-DOWN-PLAN S31). Only its own process
//! group ends with it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_sandbox::{Platform, SpawnSpec};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, watch};

use crate::model::Artifact;
use crate::record::{Direction, Recorder};
use crate::redact::Redactor;
use crate::{Error, Ledger, Result};

/// Stdout lines buffered between the reader and the adapter.
const STDOUT_LINES: usize = 256;
const STDERR_TAIL_LINES: usize = 40;
/// After a forced kill, how long to wait for the child to be reaped.
const REAP_WAIT: Duration = Duration::from_secs(2);
/// How often the tracker walks the CLI's process tree.
const TRACK_EVERY: Duration = Duration::from_secs(1);

/// Process groups no CLI's folder sweep ends: the daemon's previews, which a worker starts in
/// its own folder through Brigadier and which outlive its CLI. Group ids are system-wide, so one
/// list serves every CLI.
static SPARED: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Keeps the process group led by a pid, and what runs below that leader, out of every CLI's
/// folder sweep until dropped.
pub struct Spared(u32);

/// Spares the process group `leader` leads from folder sweeps while the result is held.
pub fn spare(leader: u32) -> Spared {
    lock(&SPARED).push(leader);
    Spared(leader)
}

impl Drop for Spared {
    fn drop(&mut self) {
        let mut spared = lock(&SPARED);
        if let Some(at) = spared.iter().position(|leader| *leader == self.0) {
            spared.swap_remove(at);
        }
    }
}

/// How a CLI process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    /// The signal that ended it, when one did (a `kill -9`, a crash).
    pub signal: Option<i32>,
}

pub struct CliProcess {
    private_logs: bool,
    pid: u32,
    started_at_ms: Option<f64>,
    platform: Arc<dyn Platform>,
    stdin: tokio::sync::Mutex<Option<ChildStdin>>,
    exit: watch::Receiver<Option<Exit>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// How many lines it has written to stderr.
    stderr_lines: watch::Receiver<u64>,
    recorder: Option<Arc<Recorder>>,
    tree: Arc<Tree>,
}

/// What else a spawned CLI process is wired to.
#[derive(Default)]
pub struct Options {
    /// A cleanup control connection can be private without recording more artifacts.
    pub private_logs: bool,
    /// Records the stdio exchange.
    pub recorder: Option<Arc<Recorder>>,
    /// Redacts stderr lines before they are logged or kept.
    pub redactor: Option<Arc<Redactor>>,
    /// Records the group and session leaders the CLI starts, for the crash sweep.
    pub ledger: Option<Arc<dyn Ledger>>,
    /// A folder that is this CLI's alone ([`SessionSpec::sweep_dir`](crate::SessionSpec::sweep_dir)),
    /// not necessarily its working directory: several CLIs may share a working directory. Whatever
    /// still runs inside it when the CLI goes is ended too: that finds a server that detached
    /// from the tree before a walk saw it.
    pub sweep_dir: Option<PathBuf>,
    /// What its commands leave running outlives it: nothing below it is tracked, recorded or
    /// ended but its own process group (S31).
    pub keeps_detached: bool,
}

/// The processes seen below the CLI, by pid, with their start times.
struct Tree {
    private_logs: bool,
    root: u32,
    platform: Arc<dyn Platform>,
    seen: Mutex<HashMap<u32, Option<f64>>>,
    sweep_dir: Option<PathBuf>,
    /// Nothing is tracked or ended ([`Options::keeps_detached`]).
    keeps_detached: bool,
}

impl Tree {
    /// Walks the CLI's tree and the trees of every process seen before (a detached server is
    /// no longer the CLI's descendant, but what it starts is its own). Returns the leaders of
    /// process groups among the processes seen for the first time.
    fn scan(&self) -> Vec<(u32, Option<f64>)> {
        if self.keeps_detached {
            return Vec::new();
        }
        let processes = self.platform.processes();
        let mut roots = vec![self.root];
        {
            // Forget what exited: its pid may be reused by a process that is not ours.
            let mut seen = lock(&self.seen);
            seen.retain(|pid, started| is_same(processes, *pid, *started));
            roots.extend(seen.keys().copied());
        }
        let mut found = Vec::new();
        for root in roots {
            if let Ok(below) = processes.descendants(root) {
                found.extend(below);
            }
        }
        let mut leaders = Vec::new();
        let mut seen = lock(&self.seen);
        for pid in found {
            if pid == self.root || seen.contains_key(&pid) {
                continue;
            }
            let started = processes.start_time_ms(pid).ok();
            seen.insert(pid, started);
            if processes.group_of(pid) == Some(pid) {
                leaders.push((pid, started));
            }
        }
        leaders
    }

    /// Kills every process seen that still runs and is still the one seen (same start time),
    /// with its own tree and group, then whatever still works inside the swept folder but a
    /// spared group's ([`spare`]). Returns how many were running.
    fn end_all(&self) -> usize {
        if self.keeps_detached {
            return 0;
        }
        let processes = self.platform.processes();
        let seen: Vec<(u32, Option<f64>)> = lock(&self.seen).drain().collect();
        let mut ended = 0;
        for (pid, started) in seen {
            if is_same(processes, pid, started) {
                if let Err(err) = processes.kill_tree(pid) {
                    crate::logging::private_trace!(self.private_logs, debug, "could not end a process the cli left"; pid, error = %err);
                }
                ended += 1;
            }
        }
        if let Some(dir) = &self.sweep_dir {
            match processes.in_dir(dir) {
                Ok(pids) => {
                    let leaders = lock(&SPARED).clone();
                    let mut spared: HashSet<u32> = leaders.iter().copied().collect();
                    for leader in &leaders {
                        spared.extend(processes.descendants(*leader).unwrap_or_default());
                    }
                    for pid in pids {
                        if spared.contains(&pid)
                            || processes
                                .group_of(pid)
                                .is_some_and(|group| leaders.contains(&group))
                        {
                            continue;
                        }
                        if let Err(err) = processes.kill_tree(pid) {
                            crate::logging::private_trace!(self.private_logs, debug, "could not end a process in its folder"; pid, error = %err);
                        }
                        ended += 1;
                    }
                }
                Err(err) => {
                    crate::logging::private_trace!(self.private_logs, debug, "no folder sweep here"; dir = %dir.display(), error = %err)
                }
            }
        }
        ended
    }
}

/// A running CLI and the lines it prints.
pub struct Spawned {
    pub process: Arc<CliProcess>,
    pub stdout: mpsc::Receiver<String>,
}

/// Spawns `spec` with piped stdio in its own process group.
pub fn spawn(platform: Arc<dyn Platform>, spec: &SpawnSpec, options: Options) -> Result<Spawned> {
    let Options {
        private_logs,
        recorder,
        redactor,
        ledger,
        sweep_dir,
        keeps_detached,
    } = options;
    let private_logs = private_logs || ledger.as_ref().is_some_and(|ledger| ledger.private_logs());
    let mut command = tokio::process::Command::from(platform.processes().piped_command(spec));
    command.stdin(Stdio::piped()).kill_on_drop(false);
    let mut child = command
        .spawn()
        .map_err(|err| Error::Spawn(format!("{}: {err}", spec.program.display())))?;
    let pid = child
        .id()
        .ok_or_else(|| Error::Spawn("the process exited immediately".into()))?;
    let started_at_ms = platform.processes().start_time_ms(pid).ok();
    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    let (lines_tx, lines) = mpsc::channel(STDOUT_LINES);
    let out_recorder = recorder.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if let Some(recorder) = &out_recorder {
                recorder.record(Direction::Out, &line);
            }
            if lines_tx.send(line).await.is_err() {
                break;
            }
        }
    });

    let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
    let tail = stderr_tail.clone();
    let (stderr_count, stderr_lines) = watch::channel(0);
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(mut line)) = reader.next_line().await {
            if let Some(redactor) = &redactor {
                redactor.redact_in_place(&mut line);
            }
            if private_logs {
                tracing::debug!(pid, stderr = "tab chat", "cli stderr");
            } else {
                tracing::debug!(pid, stderr = %line, "cli stderr");
            }
            let mut tail = tail.lock().unwrap_or_else(|p| p.into_inner());
            if tail.len() == STDERR_TAIL_LINES {
                tail.pop_front();
            }
            tail.push_back(line);
            drop(tail);
            stderr_count.send_modify(|count| *count += 1);
        }
    });

    let tree = Arc::new(Tree {
        private_logs,
        root: pid,
        platform: platform.clone(),
        seen: Mutex::new(HashMap::new()),
        sweep_dir,
        keeps_detached,
    });
    let (exit_tx, exit) = watch::channel(None);
    let tracked = tree.clone();
    let mut until_exit = exit.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(TRACK_EVERY);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = every.tick() => {}
                _ = until_exit.wait_for(Option::is_some) => return,
            }
            for (leader, started_at_ms) in tracked.scan() {
                if let Some(ledger) = &ledger {
                    let artifact = Artifact::Process {
                        pid: leader,
                        started_at_ms,
                    };
                    if let Err(err) = ledger.record(artifact).await {
                        crate::logging::private_trace!(private_logs, warn, "could not record a process"; pid = leader, error = %err);
                    }
                }
            }
        }
    });

    let reaper = platform.clone();
    let left = tree.clone();
    tokio::spawn(async move {
        let status = child.wait().await;
        // The CLI may have left children behind in its group (background shells, servers),
        // and in groups of their own.
        let _ = reaper.processes().kill_group(pid);
        let ended = left.end_all();
        let code = status.as_ref().ok().and_then(|status| status.code());
        let signal = status.ok().and_then(exit_signal);
        tracing::debug!(pid, code, signal, ended, "cli process exited");
        exit_tx.send_replace(Some(Exit { code, signal }));
    });

    Ok(Spawned {
        process: Arc::new(CliProcess {
            private_logs,
            pid,
            started_at_ms,
            platform,
            stdin: tokio::sync::Mutex::new(stdin),
            exit,
            stderr_tail,
            stderr_lines,
            recorder,
            tree,
        }),
        stdout: lines,
    })
}

impl CliProcess {
    pub(crate) fn private_logs(&self) -> bool {
        self.private_logs
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn started_at_ms(&self) -> Option<f64> {
        self.started_at_ms
    }

    pub fn is_running(&self) -> bool {
        self.exit.borrow().is_none()
    }

    /// Writes one line (a newline is appended) to the CLI's stdin.
    pub async fn write_line(&self, line: &str) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        let pipe = stdin.as_mut().ok_or(Error::Closed)?;
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        if let Err(err) = pipe.write_all(&bytes).await {
            *stdin = None;
            return Err(if err.kind() == std::io::ErrorKind::BrokenPipe {
                Error::Closed
            } else {
                err.into()
            });
        }
        pipe.flush().await?;
        if let Some(recorder) = &self.recorder {
            recorder.record(Direction::In, line);
        }
        Ok(())
    }

    /// Closes the CLI's stdin: a one-shot command that reads its input from stdin starts once
    /// it sees the end.
    pub async fn close_stdin(&self) {
        self.stdin.lock().await.take();
    }

    /// Resolves once the process has exited and been reaped.
    pub async fn exited(&self) -> Exit {
        let mut exit = self.exit.clone();
        match exit.wait_for(Option::is_some).await {
            Ok(exit) => exit.expect("checked by wait_for"),
            // The waiter task is gone; the process was reaped with it.
            Err(_) => Exit {
                code: None,
                signal: None,
            },
        }
    }

    /// Changes each time the CLI writes a line to stderr (its tail has it by then).
    pub fn stderr_lines(&self) -> watch::Receiver<u64> {
        self.stderr_lines.clone()
    }

    /// The last lines the CLI wrote to stderr.
    pub fn stderr_tail(&self) -> Option<String> {
        let tail = self.stderr_tail.lock().unwrap_or_else(|p| p.into_inner());
        (!tail.is_empty()).then(|| tail.iter().cloned().collect::<Vec<_>>().join("\n"))
    }

    /// Ends the process: closes stdin (both CLIs exit on EOF), waits up to `grace`, then kills
    /// its process tree and waits for the reap. Every process it started ends with it (see the
    /// module docs). Bounded by `grace` plus [`REAP_WAIT`].
    pub async fn shutdown(&self, grace: Duration) -> Exit {
        // What runs right now, including what started since the last walk, is ended with it.
        if self.is_running() {
            self.tree.scan();
        }
        self.stdin.lock().await.take();
        if let Ok(exit) = tokio::time::timeout(grace, self.exited()).await {
            return exit;
        }
        tracing::info!(
            pid = self.pid,
            "cli did not exit in time; killing its process tree"
        );
        if let Err(err) = self.platform.processes().kill_tree(self.pid) {
            crate::logging::private_trace!(self.private_logs, warn, "could not kill the cli process tree"; pid = self.pid, error = %err);
        }
        tokio::time::timeout(REAP_WAIT, self.exited())
            .await
            .unwrap_or(Exit {
                code: None,
                signal: None,
            })
    }
}

/// Output of a short-lived CLI command.
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Runs a short CLI command (`--version`, `auth status`) to completion, killing its process
/// group if it takes longer than `timeout`.
pub async fn run(
    platform: &Arc<dyn Platform>,
    spec: &SpawnSpec,
    timeout: Duration,
) -> Result<Output> {
    let mut command = tokio::process::Command::from(platform.processes().piped_command(spec));
    command.stdin(Stdio::null());
    let child = command
        .spawn()
        .map_err(|err| Error::Spawn(format!("{}: {err}", spec.program.display())))?;
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(output) => {
            let output = output?;
            Ok(Output {
                code: output.status.code(),
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
        Err(_) => {
            if let Some(pid) = pid {
                let _ = platform.processes().kill_tree(pid);
            }
            Err(Error::Timeout("the command"))
        }
    }
}

/// Whether `pid` still runs and is the process that started at `started` (a process whose start
/// time was never known cannot be told apart from a newcomer, so it never is).
fn is_same(processes: &dyn brigadier_sandbox::Processes, pid: u32, started: Option<f64>) -> bool {
    let Some(started) = started else {
        return false;
    };
    processes.is_alive(pid)
        && processes
            .start_time_ms(pid)
            .is_ok_and(|now| (now - started).abs() < 1_000.0)
}

/// The signal that ended a process, when one did.
#[cfg(unix)]
fn exit_signal(status: std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: std::process::ExitStatus) -> Option<i32> {
    None
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Two CLIs started in one folder (a session's Claude workers share theirs): ending one
    /// ends only what runs in its own swept folder, never the other CLI. Sweeping the shared
    /// folder instead is exactly what killed the other workers at every landing.
    #[tokio::test(flavor = "multi_thread")]
    async fn ending_a_cli_spares_another_in_the_same_folder() {
        let root = std::env::temp_dir().join(format!("brigadier-sweep-{}", uuid::Uuid::new_v4()));
        let shared = root.join("shared");
        let own = root.join("own");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&own).unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        let start = |program: &str, arg: Option<&str>, sweep_dir: Option<PathBuf>| {
            let mut spec = SpawnSpec::new(program);
            if let Some(arg) = arg {
                spec = spec.arg(arg);
            }
            spec.cwd = Some(shared.clone());
            spawn(
                platform.clone(),
                &spec,
                Options {
                    sweep_dir,
                    ..Options::default()
                },
            )
            .unwrap()
            .process
        };

        let other = start("sleep", Some("30"), None);
        let first = start("cat", None, Some(own.clone()));
        first.shutdown(Duration::from_secs(5)).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            other.is_running(),
            "a CLI ending swept another one's folder"
        );

        // The old behaviour, the shared folder swept, does kill it: the test tells them apart.
        let sweeper = start("cat", None, Some(shared.clone()));
        sweeper.shutdown(Duration::from_secs(5)).await;
        let ended = tokio::time::timeout(Duration::from_secs(5), other.exited()).await;
        assert!(
            ended.is_ok(),
            "sweeping the shared folder should end the other CLI"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// A spared group (a worker's preview, run by the daemon in the worker's folder) outlives
    /// the sweep of that folder when the worker's CLI ends; once no longer spared, it doesn't.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_folder_sweep_spares_a_spared_group() {
        let root = std::env::temp_dir().join(format!("brigadier-spare-{}", uuid::Uuid::new_v4()));
        let own = root.join("own");
        std::fs::create_dir_all(&own).unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        let start = |program: &str, arg: Option<&str>, sweep_dir: Option<PathBuf>| {
            let mut spec = SpawnSpec::new(program);
            if let Some(arg) = arg {
                spec = spec.arg(arg);
            }
            spec.cwd = Some(own.clone());
            spawn(
                platform.clone(),
                &spec,
                Options {
                    sweep_dir,
                    ..Options::default()
                },
            )
            .unwrap()
            .process
        };

        let preview = start("sleep", Some("30"), None);
        let spared = spare(preview.pid());
        let cli = start("cat", None, Some(own.clone()));
        cli.shutdown(Duration::from_secs(5)).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(preview.is_running(), "the sweep ended a spared group");

        drop(spared);
        let cli = start("cat", None, Some(own.clone()));
        cli.shutdown(Duration::from_secs(5)).await;
        let ended = tokio::time::timeout(Duration::from_secs(5), preview.exited()).await;
        assert!(
            ended.is_ok(),
            "the sweep should end a group no longer spared"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// S31: what a session's CLI leaves running, detached and seen by a walk, outlives the
    /// CLI; any other CLI's ends with it.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn only_a_cli_that_keeps_detached_leaves_its_commands_running() {
        let root = std::env::temp_dir().join(format!("brigadier-keep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        for keeps_detached in [true, false] {
            // A command in a session of its own, as both CLIs start them; the CLI stays past a
            // walk of its tree, then exits.
            let spec = SpawnSpec::new("python3").arg("-c").arg(
                "import subprocess, sys, time\n\
                 child = subprocess.Popen(['sleep', '60'], start_new_session=True)\n\
                 print(child.pid, flush=True)\n\
                 time.sleep(2.5)",
            );
            let Spawned {
                process,
                mut stdout,
            } = spawn(
                platform.clone(),
                &spec,
                Options {
                    keeps_detached,
                    ..Options::default()
                },
            )
            .unwrap();
            let pid = stdout.recv().await.unwrap().trim().to_owned();
            process.exited().await;
            tokio::time::sleep(Duration::from_millis(300)).await;
            let kill = |signal: &str| {
                std::process::Command::new("kill")
                    .args([signal, pid.as_str()])
                    .stderr(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            };
            let running = kill("-0");
            if running {
                kill("-9");
            }
            assert_eq!(running, keeps_detached, "keeps_detached: {keeps_detached}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
