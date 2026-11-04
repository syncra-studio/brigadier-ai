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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_sandbox::{Platform, SpawnSpec};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::model::Artifact;
use crate::record::{Direction, Recorder};
use crate::redact::Redactor;
use crate::{Error, Ledger, Result};

/// Stdout lines buffered between the reader and the adapter.
const STDOUT_LINES: usize = 256;
/// Stdout bytes buffered between the reader and the adapter, before the reader waits ([`Queued`]).
const STDOUT_QUEUED_BYTES: usize = 8 * 1024 * 1024;
/// The longest stdout line read; a CLI that prints a longer one is ended (#769). The longest one
/// a CLI prints in normal use is the largest message Brigadier sends, echoed whole: Claude
/// replays each (`--replay-user-messages`) with its images inline, base64-encoded, so 20
/// attachments of 10 MiB take some 267 MiB. Core checks at build time that its attachment limits
/// fit (`MAX_ATTACHMENT_BYTES` in `core/src/sessions.rs`).
pub const STDOUT_LINE_MAX: usize = 320 * 1024 * 1024;
/// The longest stderr line kept; the rest of a longer one is dropped.
const STDERR_LINE_MAX: usize = 64 * 1024;
const STDERR_TAIL_LINES: usize = 40;
/// After a forced kill, how long to wait for the child to be reaped.
const REAP_WAIT: Duration = Duration::from_secs(2);
/// How often the tracker walks the CLI's process tree.
const TRACK_EVERY: Duration = Duration::from_secs(1);
/// How long one write to the CLI's stdin may take, waiting for the one before it included. Both
/// CLIs read their input all along, so a write that takes longer meets a CLI that stopped
/// reading: it is ended rather than left to hold Stop and close forever (#755).
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

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
    write_timeout: Duration,
    exit: watch::Receiver<Option<Exit>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    /// How many lines it has written to stderr.
    stderr_lines: watch::Receiver<u64>,
    recorder: Option<Arc<Recorder>>,
    tree: Arc<Tree>,
    fault: Arc<Fault>,
}

/// Ends a CLI whose stdio protocol can no longer be trusted: a line to its stdin was cut short
/// (#755), or it printed a line too long to read (#769). Its stderr tail says why, so the
/// session's exit reports it, and its process tree is killed, which ends its stdout: the
/// adapter's read loop then ends the session and whatever turn it ran.
struct Fault {
    private_logs: bool,
    pid: u32,
    platform: Arc<dyn Platform>,
    exit: watch::Receiver<Option<Exit>>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    ended: AtomicBool,
}

impl Fault {
    fn end(&self, reason: &str) {
        if self.ended.swap(true, Ordering::AcqRel) || self.exit.borrow().is_some() {
            return;
        }
        tracing::warn!(pid = self.pid, reason, "ending a cli whose stdio broke");
        push_tail(
            &self.stderr_tail,
            format!("Brigadier ended the CLI: {reason}."),
        );
        if let Err(err) = self.platform.processes().kill_tree(self.pid) {
            crate::logging::private_trace!(self.private_logs, warn, "could not kill the cli process tree"; pid = self.pid, error = %err);
        }
    }
}

/// Ends the CLI ([`Fault`]) when dropped armed: a write cut short by its time limit, or by its
/// caller dropping it, may have sent part of a line, and the CLI reads the next one appended.
struct Unfinished<'a>(Option<&'a Fault>);

impl Drop for Unfinished<'_> {
    fn drop(&mut self) {
        if let Some(fault) = self.0.take() {
            fault.end("a line to its input was cut short");
        }
    }
}

fn push_tail(tail: &Mutex<VecDeque<String>>, line: String) {
    let mut tail = lock(tail);
    if tail.len() == STDERR_TAIL_LINES {
        tail.pop_front();
    }
    tail.push_back(line);
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
    /// How long one write to its stdin may take before the CLI is ended; `None` is
    /// [`WRITE_TIMEOUT`].
    pub write_timeout: Option<Duration>,
    /// Its stdout carries logs, not the protocol (`opencode serve`): a line longer than a
    /// stderr line is cut short, rather than ending the CLI.
    pub stdout_is_log: bool,
    /// The longest stdout line read before the CLI is ended; `None` is [`STDOUT_LINE_MAX`].
    pub stdout_line_max: Option<usize>,
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
    pub stdout: Lines,
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
        write_timeout,
        stdout_is_log,
        stdout_line_max,
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

    let stderr_tail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
    let (exit_tx, exit) = watch::channel(None);
    let fault = Arc::new(Fault {
        private_logs,
        pid,
        platform: platform.clone(),
        exit: exit.clone(),
        stderr_tail: stderr_tail.clone(),
        ended: AtomicBool::new(false),
    });

    let (lines_tx, lines) = mpsc::channel(STDOUT_LINES);
    let queued = Arc::new(Queued::new(STDOUT_QUEUED_BYTES));
    let out = StdoutPump {
        max: if stdout_is_log {
            STDERR_LINE_MAX
        } else {
            stdout_line_max.unwrap_or(STDOUT_LINE_MAX)
        },
        is_log: stdout_is_log,
        queued: queued.clone(),
        recorder: recorder.clone(),
        lines: lines_tx,
    };
    let out_fault = fault.clone();
    tokio::spawn(out.run(BufReader::new(stdout), move |max| {
        out_fault.end(&format!(
            "it printed a line longer than the {} Brigadier reads",
            size(max)
        ));
    }));

    let tail = stderr_tail.clone();
    let (stderr_count, stderr_lines) = watch::channel(0);
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        loop {
            let (line, cut) = match read_line(&mut reader, STDERR_LINE_MAX).await {
                Ok(Read::Line(line)) => (line, false),
                // Its start is enough for an error report; the rest is dropped.
                Ok(Read::TooLong(line)) => (line, true),
                Ok(Read::End) | Err(_) => break,
            };
            let line = stderr_line(line, cut, redactor.as_deref());
            if private_logs {
                tracing::debug!(pid, stderr = "tab chat", "cli stderr");
            } else {
                tracing::debug!(pid, stderr = %line, "cli stderr");
            }
            push_tail(&tail, line);
            stderr_count.send_modify(|count| *count += 1);
            if cut && !matches!(skip_line(&mut reader).await, Ok(true)) {
                break;
            }
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
            write_timeout: write_timeout.unwrap_or(WRITE_TIMEOUT),
            exit,
            stderr_tail,
            stderr_lines,
            recorder,
            tree,
            fault,
        }),
        stdout: Lines { lines, queued },
    })
}

/// The lines a CLI prints on stdout, in order, as the adapter takes them.
pub struct Lines {
    lines: mpsc::Receiver<String>,
    queued: Arc<Queued>,
}

impl Lines {
    /// The next line; `None` once stdout ended, or the CLI was ended for what it printed.
    pub async fn recv(&mut self) -> Option<String> {
        let line = self.lines.recv().await?;
        self.queued.take(line.len());
        Some(line)
    }

    /// The next line if one is ready.
    pub fn try_recv(&mut self) -> std::result::Result<String, mpsc::error::TryRecvError> {
        let line = self.lines.try_recv()?;
        self.queued.take(line.len());
        Ok(line)
    }
}

/// Stdout bytes read but not taken by the adapter yet. The reader starts no new line while
/// they reach the budget, so what a CLI's output holds in memory is at most the budget plus the
/// line being read (#769).
struct Queued {
    budget: usize,
    bytes: AtomicUsize,
    taken: tokio::sync::Notify,
}

impl Queued {
    fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: AtomicUsize::new(0),
            taken: tokio::sync::Notify::new(),
        }
    }

    async fn room(&self) {
        while self.bytes.load(Ordering::Acquire) >= self.budget {
            self.taken.notified().await;
        }
    }

    // An empty line counts as a byte: a flood of them is bounded too.
    fn add(&self, len: usize) {
        self.bytes.fetch_add(len + 1, Ordering::AcqRel);
    }

    fn take(&self, len: usize) {
        self.bytes.fetch_sub(len + 1, Ordering::AcqRel);
        // Stored when the reader isn't waiting yet, so it never misses one.
        self.taken.notify_one();
    }
}

/// Reads a CLI's stdout into [`Lines`].
struct StdoutPump {
    max: usize,
    /// Logs, not the protocol ([`Options::stdout_is_log`]).
    is_log: bool,
    queued: Arc<Queued>,
    recorder: Option<Arc<Recorder>>,
    lines: mpsc::Sender<String>,
}

impl StdoutPump {
    /// Hands each line on until stdout ends or the adapter goes. A protocol line longer than
    /// `max` is never handed on cut short: the adapter could wait forever on what it held (a
    /// turn's end), so `too_long` ends the CLI instead, and the lines end with it.
    async fn run<R: AsyncBufRead + Unpin>(self, mut reader: R, too_long: impl FnOnce(usize)) {
        loop {
            self.queued.room().await;
            let line = match read_line(&mut reader, self.max).await {
                Ok(Read::Line(line)) => line,
                Ok(Read::TooLong(line)) if self.is_log => {
                    if !matches!(skip_line(&mut reader).await, Ok(true)) {
                        break;
                    }
                    line
                }
                Ok(Read::TooLong(_)) => {
                    too_long(self.max);
                    drop(self.lines);
                    // Read to the end, so a CLI not ended yet never blocks on a full pipe.
                    while matches!(skip_line(&mut reader).await, Ok(true)) {}
                    return;
                }
                Ok(Read::End) | Err(_) => break,
            };
            if let Some(recorder) = &self.recorder {
                recorder.record(Direction::Out, &line);
            }
            self.queued.add(line.len());
            if self.lines.send(line).await.is_err() {
                break;
            }
        }
    }
}

enum Read {
    Line(String),
    /// The line's first `max` bytes; the rest is still to read.
    TooLong(String),
    End,
}

/// Reads one line, without its line ending, its invalid UTF-8 replaced: a byte that isn't text
/// no longer ends the reader (#769). Stops at `max` bytes ([`Read::TooLong`]).
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R, max: usize) -> std::io::Result<Read> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(Read::End);
            }
            break;
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.unwrap_or(available.len());
        if line.len() + take > max {
            let room = max - line.len();
            line.extend_from_slice(&available[..room]);
            reader.consume(room);
            without_cut_char(&mut line);
            return Ok(Read::TooLong(text(line)));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take + usize::from(newline.is_some()));
        if newline.is_some() {
            break;
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(Read::Line(text(line)))
}

/// Drops the rest of a line. Whether a line followed (`false` at the end).
async fn skip_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> std::io::Result<bool> {
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(false);
        }
        if let Some(at) = available.iter().position(|byte| *byte == b'\n') {
            reader.consume(at + 1);
            return Ok(true);
        }
        let len = available.len();
        reader.consume(len);
    }
}

/// Drops a character the limit cut in two from the end of `bytes`: decoded, it would end in a
/// replacement character, and a secret cut there would no longer read as the start of one.
fn without_cut_char(bytes: &mut Vec<u8>) {
    let from = bytes.len().saturating_sub(3);
    let Some(lead) = (from..bytes.len())
        .rev()
        .find(|at| bytes[*at] & 0xC0 != 0x80)
    else {
        return;
    };
    let width = match bytes[lead] {
        byte if byte >= 0xF0 => 4,
        byte if byte >= 0xE0 => 3,
        byte if byte >= 0xC0 => 2,
        _ => 1,
    };
    if bytes.len() - lead < width {
        bytes.truncate(lead);
    }
}

/// A stderr line as it is logged and kept, its secrets redacted. One cut at the limit also loses
/// the tail that could be the start of a secret: the rest of that secret, which would have
/// redacted it, was dropped with the rest of the line.
fn stderr_line(line: String, cut: bool, redactor: Option<&Redactor>) -> String {
    let mut line = match redactor {
        Some(redactor) if cut => redactor.push(&mut String::new(), &line).into_owned(),
        Some(redactor) => redactor.redact(&line).into_owned(),
        None => line,
    };
    if cut {
        line.push_str(" …");
    }
    line
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into_owned())
}

fn size(bytes: usize) -> String {
    const MIB: usize = 1024 * 1024;
    if bytes >= MIB {
        format!("{} MiB", bytes / MIB)
    } else {
        format!("{} KiB", bytes.div_ceil(1024))
    }
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

    /// Writes one line (a newline is appended) to the CLI's stdin, within the write time limit
    /// ([`Options::write_timeout`]), waiting for an earlier write included. A write that runs
    /// out of time, or is dropped partway, ends the CLI ([`Fault`]): part of a line may be in the
    /// pipe already, and the CLI would read the next line appended to it (#755).
    pub async fn write_line(&self, line: &str) -> Result<()> {
        let deadline = Instant::now() + self.write_timeout;
        // An earlier write holds stdin; it ends the CLI if it outlasts its own limit, which
        // comes before this one's.
        let Ok(mut stdin) = tokio::time::timeout_at(deadline, self.stdin.lock()).await else {
            return Err(Error::Timeout("the CLI to read its input"));
        };
        let pipe = stdin.as_mut().ok_or(Error::Closed)?;
        let mut bytes = Vec::with_capacity(line.len() + 1);
        bytes.extend_from_slice(line.as_bytes());
        bytes.push(b'\n');
        let mut unfinished = Unfinished(Some(&self.fault));
        let written = async {
            pipe.write_all(&bytes).await?;
            pipe.flush().await
        };
        let written = tokio::time::timeout_at(deadline, written).await;
        match written {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                // The pipe broke: the CLI is gone or going, and reads nothing more.
                unfinished.0 = None;
                *stdin = None;
                return Err(if err.kind() == std::io::ErrorKind::BrokenPipe {
                    Error::Closed
                } else {
                    err.into()
                });
            }
            Err(_) => {
                *stdin = None;
                drop(unfinished);
                return Err(Error::Timeout("the CLI to read its input"));
            }
        }
        unfinished.0 = None;
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
    /// module docs). Bounded by `grace` plus [`REAP_WAIT`]: a write the CLI doesn't read, which
    /// holds stdin open, holds it no longer than `grace` (#755).
    pub async fn shutdown(&self, grace: Duration) -> Exit {
        let deadline = Instant::now() + grace;
        // What runs right now, including what started since the last walk, is ended with it.
        if self.is_running() {
            self.tree.scan();
        }
        if let Ok(mut stdin) = tokio::time::timeout_at(deadline, self.stdin.lock()).await {
            stdin.take();
        }
        if let Ok(exit) = tokio::time::timeout_at(deadline, self.exited()).await {
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

/// Whether a process with this id exists, for files a CLI keeps only while it runs: a process
/// of another user's counts. Where that can't be told, every process does.
pub(crate) fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::sys::signal::kill;
        use nix::unistd::Pid;
        let Some(pid) = i32::try_from(pid).ok().filter(|pid| *pid > 0) else {
            return false;
        };
        // Signal 0 sends nothing: it only checks that the process exists.
        match kill(Pid::from_raw(pid), None) {
            Ok(()) => true,
            Err(err) => err != nix::errno::Errno::ESRCH,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Pumps `input` as a CLI's stdout, read 7 bytes at a time so lines and characters span
    /// reads. Returns its lines, what is queued, and the limit a line passed, if one did.
    fn pump(
        input: Vec<u8>,
        max: usize,
        budget: usize,
        is_log: bool,
    ) -> (Lines, Arc<Queued>, Arc<Mutex<Option<usize>>>) {
        let (lines_tx, lines) = mpsc::channel(STDOUT_LINES);
        let queued = Arc::new(Queued::new(budget));
        let passed = Arc::new(Mutex::new(None));
        let pump = StdoutPump {
            max,
            is_log,
            queued: queued.clone(),
            recorder: None,
            lines: lines_tx,
        };
        let reader = BufReader::with_capacity(7, std::io::Cursor::new(input));
        let seen = passed.clone();
        tokio::spawn(pump.run(reader, move |max| *lock(&seen) = Some(max)));
        (
            Lines {
                lines,
                queued: queued.clone(),
            },
            queued,
            passed,
        )
    }

    async fn all(lines: &mut Lines) -> Vec<String> {
        let mut all = Vec::new();
        while let Some(line) = lines.recv().await {
            all.push(line);
        }
        all
    }

    /// #769: a byte that isn't UTF-8 no longer ends the reader (and with it the pipe): the line
    /// reads with it replaced, and the lines after it still come.
    #[tokio::test]
    async fn a_byte_that_is_not_text_does_not_end_the_output() {
        let input = b"one\n\xff\xfe two\r\nthr\xc3\xa9e\n\xe2\x82\nlast".to_vec();
        let (mut lines, _, passed) = pump(input, 1024, 1024, false);
        assert_eq!(
            all(&mut lines).await,
            [
                "one",
                "\u{fffd}\u{fffd} two",
                "thr\u{e9}e",
                "\u{fffd}",
                "last"
            ]
        );
        assert_eq!(*lock(&passed), None);
    }

    /// #769: a protocol line longer than the limit is never handed on cut short (it may be a
    /// turn's end, which the adapter would wait on forever): the CLI is ended, and the lines end.
    #[tokio::test]
    async fn a_protocol_line_too_long_ends_the_cli_instead_of_being_cut() {
        let mut input = b"short\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', 100));
        input.extend(b"\nafter\n");
        let (mut lines, _, passed) = pump(input, 64, 1024, false);
        assert_eq!(all(&mut lines).await, ["short"]);
        assert_eq!(*lock(&passed), Some(64));
    }

    /// #769: on a CLI whose stdout is logs, a long line is cut short and reading goes on, as on
    /// stderr.
    #[tokio::test]
    async fn a_log_line_too_long_is_cut_short() {
        let mut input = b"short\n".to_vec();
        input.extend(std::iter::repeat_n(b'x', 100));
        input.extend(b"\nafter\n");
        let (mut lines, _, passed) = pump(input, 64, 1024, true);
        assert_eq!(all(&mut lines).await, ["short", &"x".repeat(64), "after"]);
        assert_eq!(*lock(&passed), None);
    }

    #[tokio::test]
    async fn a_line_reads_up_to_its_limit_and_skips_the_rest() {
        let mut reader =
            BufReader::with_capacity(3, std::io::Cursor::new(b"abcdefg\nhi\n".to_vec()));
        assert!(
            matches!(read_line(&mut reader, 4).await.unwrap(), Read::TooLong(line) if line == "abcd")
        );
        assert!(skip_line(&mut reader).await.unwrap());
        assert!(
            matches!(read_line(&mut reader, 4).await.unwrap(), Read::Line(line) if line == "hi")
        );
        assert!(matches!(
            read_line(&mut reader, 4).await.unwrap(),
            Read::End
        ));
        assert!(!skip_line(&mut reader).await.unwrap());
    }

    /// #769: what the reader holds is bounded in bytes, not lines: while the adapter takes
    /// nothing, it starts no line once the budget is reached, and goes on as lines are taken.
    #[tokio::test]
    async fn the_reader_waits_while_its_byte_budget_is_used() {
        let (mut lines, queued, _) = pump(b"aaaa\n".repeat(100), 1024, 12, false);
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Three lines of 4 bytes, each counted with its end: 15 reaches the budget of 12.
        assert_eq!(queued.bytes.load(Ordering::Acquire), 15);
        let all = all(&mut lines).await;
        assert_eq!(all.len(), 100);
        assert_eq!(queued.bytes.load(Ordering::Acquire), 0);
    }

    /// A secret cut at the stderr line limit shows no part of itself: the rest of it, which
    /// would have matched, is dropped with the rest of the line, so the tail that could start
    /// one goes too. A whole one before the cut is redacted as on any line.
    #[test]
    fn a_secret_cut_with_a_long_stderr_line_shows_no_part_of_it() {
        let redactor = Redactor::new(["supersecret-token"]);
        assert_eq!(
            stderr_line(
                "a supersecret-token b supersecret-tok".into(),
                true,
                Some(&redactor)
            ),
            "a [redacted] b  …"
        );
        assert_eq!(
            stderr_line(
                "a supersecret-token b supers".into(),
                false,
                Some(&redactor)
            ),
            "a [redacted] b supers"
        );
        assert_eq!(stderr_line("plain".into(), true, None), "plain …");
    }

    /// The limit can fall inside a character: that character is dropped rather than read as a
    /// replacement character, which would hide the start of a secret cut there.
    #[tokio::test]
    async fn a_character_cut_by_the_limit_is_dropped() {
        let input = "login passwörd-123 rest\n".as_bytes().to_vec();
        // "login passw" and the first byte of "ö".
        let mut reader = BufReader::with_capacity(5, std::io::Cursor::new(input));
        let Read::TooLong(line) = read_line(&mut reader, 12).await.unwrap() else {
            panic!("the line was not cut");
        };
        assert_eq!(line, "login passw");
        let redactor = Redactor::new(["passwörd-123"]);
        assert_eq!(stderr_line(line, true, Some(&redactor)), "login  …");

        for (bytes, kept) in [
            (&b"ab\xc3"[..], &b"ab"[..]),
            (b"ab\xe2\x82", b"ab"),
            (b"ab\xf0\x9f\x98", b"ab"),
            (b"ab\xc3\xa9", b"ab\xc3\xa9"),
            (b"ab\xe2\x82\xac", b"ab\xe2\x82\xac"),
            (b"abc", b"abc"),
            (b"", b""),
        ] {
            let mut cut = bytes.to_vec();
            without_cut_char(&mut cut);
            assert_eq!(cut, kept, "{bytes:?}");
        }
    }

    /// A CLI that runs for a minute and never reads its input.
    fn deaf() -> SpawnSpec {
        if cfg!(windows) {
            SpawnSpec::new("ping").arg("-n").arg("60").arg("127.0.0.1")
        } else {
            SpawnSpec::new("sleep").arg("60")
        }
    }

    fn scratch(name: &str) -> (PathBuf, Arc<dyn Platform>) {
        let root = std::env::temp_dir().join(format!("brigadier-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(root.join("data")),
        })
        .unwrap();
        (root, platform)
    }

    /// More than any pipe buffer holds, as a pasted image is.
    fn big_line() -> String {
        "x".repeat(16 * 1024 * 1024)
    }

    /// #755: a write the CLI never reads holds stdin. Stop's interrupt, the next write, doesn't
    /// queue behind it forever: both give up within the write limit, and the CLI is ended with
    /// the reason in its stderr tail, with no shutdown asked for.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_write_the_cli_never_reads_ends_it_within_the_limit() {
        let (root, platform) = scratch("deaf-write");
        let Spawned {
            process,
            stdout: _stdout,
        } = spawn(
            platform,
            &deaf(),
            Options {
                write_timeout: Some(Duration::from_millis(500)),
                ..Options::default()
            },
        )
        .unwrap();
        let writer = tokio::spawn({
            let process = process.clone();
            async move { process.write_line(&big_line()).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let interrupt = tokio::time::timeout(
            Duration::from_secs(20),
            process.write_line(r#"{"type":"control_request","request":{"subtype":"interrupt"}}"#),
        )
        .await
        .expect("the interrupt waited forever behind a write the CLI doesn't read");
        assert!(interrupt.is_err(), "{interrupt:?}");
        let written = tokio::time::timeout(Duration::from_secs(20), writer)
            .await
            .expect("the write the CLI doesn't read never gave up")
            .unwrap();
        assert!(matches!(written, Err(Error::Timeout(_))), "{written:?}");
        tokio::time::timeout(Duration::from_secs(20), process.exited())
            .await
            .expect("the CLI was not ended");
        let tail = process.stderr_tail().unwrap_or_default();
        assert!(tail.contains("cut short"), "{tail}");
        // Ended: what is written next fails at once.
        assert!(matches!(process.write_line("{}").await, Err(Error::Closed)));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// #755: shutdown is bounded by its grace even while a write the CLI doesn't read holds
    /// stdin (with a write limit longer than the grace).
    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_does_not_wait_for_a_stuck_write() {
        let (root, platform) = scratch("deaf-shutdown");
        let Spawned {
            process,
            stdout: _stdout,
        } = spawn(
            platform,
            &deaf(),
            Options {
                write_timeout: Some(Duration::from_secs(300)),
                ..Options::default()
            },
        )
        .unwrap();
        let writer = tokio::spawn({
            let process = process.clone();
            async move { process.write_line(&big_line()).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        tokio::time::timeout(
            Duration::from_secs(20),
            process.shutdown(Duration::from_millis(500)),
        )
        .await
        .expect("shutdown waited for the stuck write");
        assert!(!process.is_running());
        // The kill broke the pipe under the write.
        let written = tokio::time::timeout(Duration::from_secs(20), writer)
            .await
            .expect("the write outlived the CLI")
            .unwrap();
        assert!(written.is_err(), "{written:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Two CLIs started in one folder (a session's Claude workers share theirs): ending one
    /// ends only what runs in its own swept folder, never the other CLI. Sweeping the shared
    /// folder instead is exactly what killed the other workers at every landing.
    #[cfg(unix)]
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
    #[cfg(unix)]
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
