//! Whether a worker's background commands still move and its builds get to run, read from the
//! machine once a minute and only for workers with something running. Detects and tells only:
//! nothing here stops or slows a process.
//!
//! - A process is a worker's by walking down from its CLI, and by its `BRIGADIER_SESSION`: what
//!   a command left behind (`( ... ) &`) has lost its parent but kept that. macOS hides the
//!   environment of system binaries such as `/bin/zsh` and `/bin/sleep`, so the marker alone
//!   would miss those.
//! - The CLI and its MCP servers are not its work: only what runs under a shell the CLI started.
//! - A job that only waits (a shell and `sleep`, a polling loop) shows no CPU of its own while
//!   what it waits for works, so it counts as moving while any other work of its worker does.
//! - What a command left behind has lost its parent, so nothing tracks it: each such tree has its
//!   own clock on CPU alone, since a watcher's output says nothing of it. An orphan whose
//!   environment the machine hides counts as the worker's when it runs in the worker's own
//!   folder. An app left running (`thread_view::persistent_app`) rests by design and is not told.
//! - Any gap between two looks longer than the machine could explain is sleep: it starts every
//!   window again, so time the machine slept never counts.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use brigadier_cli::protocol::EventKind;

use crate::model::ConversationId;
use crate::thread_view::{JobHold, JobHoldKind};
use crate::work::TaskId;

/// How long a job may show no output and almost no CPU, and a build may be held up, before its
/// worker's parent hears, unless [`STALL_ENV`] sets it in seconds.
const STALL: Duration = Duration::from_secs(10 * 60);
const STALL_ENV: &str = "BRIGADIER_WORKER_STALL_SECS";
/// Debug builds only: the machine reading as `<load>/<cores>`, to see a starved row without
/// loading the machine. Whatever it starts says it is faked.
#[cfg(debug_assertions)]
const FAKE_LOAD_ENV: &str = "BRIGADIER_FAKE_MACHINE_LOAD";
/// The share of one core below which a job counts as doing nothing.
const ALMOST_NO_CPU: f64 = 0.01;
/// The share of one core below which a compile on a loaded machine counts as starved.
const FAR_UNDER_ONE_CORE: f64 = 0.5;
/// How long a compile queues behind the slots before its row says so: queuing a few seconds
/// between crates is how a shared pool works.
const SLOT_SHOWN_AFTER_MS: i64 = 30_000;
/// The compiler launcher's name (`brigadier_sandbox::build_slots`) while it waits for a slot.
const LAUNCHER: &str = "brigadier-build-slot";
const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "cmd.exe",
    "powershell.exe",
    "pwsh",
    "pwsh.exe",
];
const WAITERS: &[&str] = &["sleep", "timeout", "gtimeout"];
const COMPILERS: &[&str] = &[
    "rustc", "cc", "c++", "clang", "clang++", "gcc", "g++", "swiftc", "ld", "lld", "compile",
];
/// The most of a command a row and a message carry.
const WHAT_CHARS: usize = 60;

/// A process as the machine says it now.
#[derive(Debug, Clone)]
pub(crate) struct Proc {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub cmd: Vec<String>,
    /// Its `BRIGADIER_SESSION`, when the machine lets it be read.
    pub marker: Option<String>,
    /// Where it runs, when the machine hides its environment and so its marker.
    pub hidden_in: Option<String>,
    pub cpu_ms: u64,
    pub started_ms: i64,
}

/// A running job of a worker's.
#[derive(Debug, Clone)]
pub(crate) struct SampledJob {
    pub id: String,
    /// How to call it: its command, else the CLI's words for it.
    pub what: String,
    /// The command line it runs, when known.
    pub command: Option<String>,
    pub started_ms: i64,
    pub output_file: Option<String>,
}

/// A worker with something running.
#[derive(Debug, Clone)]
pub(crate) struct SampledWorker {
    pub task: TaskId,
    pub conversation: ConversationId,
    pub jobs: Vec<SampledJob>,
    /// The worktree only it works in.
    pub folder: Option<String>,
}

/// A file's size and time, to see it grow.
type Output = (u64, Option<SystemTime>);

/// What a look at a worker found.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Verdict {
    pub task: TaskId,
    pub conversation: ConversationId,
    /// What holds it up now, and the job that is, when it is one job's.
    pub hold: Option<(Option<String>, JobHold)>,
    /// What its parent is to hear now.
    pub tell: Vec<(EventKind, String)>,
}

#[derive(Debug)]
struct JobClock {
    since_ms: i64,
    output: Option<Output>,
    told: bool,
}

#[derive(Debug)]
struct LeftClock {
    since_ms: i64,
    told: bool,
}

#[derive(Debug)]
struct BuildClock {
    kind: JobHoldKind,
    since_ms: i64,
    told: bool,
}

/// What the looks remember.
#[derive(Debug, Default)]
struct Clocks {
    /// When it looked last; none since nothing was left to look at.
    looked_ms: Option<i64>,
    /// When the machine last woke, or the watch began: no window starts before it.
    counted_from_ms: i64,
    cpu: HashMap<u32, u64>,
    jobs: HashMap<(TaskId, String), JobClock>,
    /// By the pid at the top of each tree a worker's commands left behind.
    left: HashMap<(TaskId, u32), LeftClock>,
    builds: HashMap<TaskId, BuildClock>,
}

/// The machine's load, for a starved build.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Machine {
    pub load: f64,
    pub cores: u32,
    /// Not the machine's own: `BRIGADIER_FAKE_MACHINE_LOAD` said it.
    pub faked: bool,
}

/// The watch of every worker's jobs.
pub(crate) struct JobWatch {
    after_ms: i64,
    clocks: Mutex<Clocks>,
    system: Mutex<sysinfo::System>,
    last_look: Mutex<Option<Instant>>,
}

impl Default for JobWatch {
    fn default() -> Self {
        let after = std::env::var(STALL_ENV)
            .ok()
            .and_then(|secs| secs.trim().parse::<u64>().ok())
            .map_or(STALL, Duration::from_secs);
        Self {
            after_ms: i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
            clocks: Mutex::default(),
            system: Mutex::new(sysinfo::System::new()),
            last_look: Mutex::new(None),
        }
    }
}

/// Its short form in a row or a message.
pub(crate) fn short_what(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let cut: String = line.chars().take(WHAT_CHARS).collect();
    if cut.len() < line.len() {
        format!("{cut}…")
    } else {
        cut
    }
}

/// A row's words for `hold` at `now`, as the app says them (`workerState.ts`).
pub(crate) fn hold_words(hold: &JobHold, now: i64) -> String {
    let minutes = now.saturating_sub(hold.since_ms) / 60_000;
    match hold.kind {
        JobHoldKind::Stalled => match &hold.what {
            Some(what) => format!("Stalled {minutes}m · {what}"),
            None => format!("Stalled {minutes}m"),
        },
        JobHoldKind::BuildSlot => format!("Waiting for a build slot {minutes}m"),
        JobHoldKind::Starved => format!(
            "Build starved {minutes}m · load {} on {} cores{}",
            hold.load.unwrap_or(0),
            hold.cores.unwrap_or(0),
            if hold.faked { " · faked" } else { "" }
        ),
    }
}

fn minutes_words(ms: i64) -> String {
    if ms >= 60_000 {
        format!("{} min", ms / 60_000)
    } else {
        format!("{} s", ms / 1000)
    }
}

fn program(cmd: &[String]) -> &str {
    cmd.first()
        .and_then(|first| Path::new(first).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("")
}

/// A process's short name, by its name or by the program it was started as.
fn is_named(proc: &Proc, names: &[&str]) -> bool {
    names.contains(&proc.name.as_str()) || names.contains(&program(&proc.cmd))
}

/// Whether `proc` is an app that rests by design. Node runs most of them as a script of its own
/// name (`node …/vite/bin/vite.js --port 1420`), so the policy reads the script as the program.
fn is_persistent_app(proc: &Proc) -> bool {
    let line = |words: &[String]| {
        words
            .iter()
            .map(|word| {
                if word.contains(char::is_whitespace) {
                    format!("'{word}'")
                } else {
                    word.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    if crate::thread_view::persistent_app(&line(&proc.cmd)) {
        return true;
    }
    let Some(([script, args @ ..], true)) = proc
        .cmd
        .split_first()
        .map(|(first, rest)| (rest, is_named_program(first, "node")))
    else {
        return false;
    };
    let script = [".mjs", ".cjs", ".js"]
        .iter()
        .find_map(|end| script.strip_suffix(end))
        .unwrap_or(script);
    let mut words = vec![script.to_owned()];
    words.extend_from_slice(args);
    crate::thread_view::persistent_app(&line(&words))
}

fn is_named_program(path: &str, name: &str) -> bool {
    Path::new(path).file_name().and_then(|file| file.to_str()) == Some(name)
}

fn is_mcp(proc: &Proc) -> bool {
    proc.cmd
        .iter()
        .any(|arg| arg.to_ascii_lowercase().contains("mcp"))
}

/// A process's command as a row says it, without its program's path.
fn command_words(proc: &Proc) -> String {
    let program = match program(&proc.cmd) {
        "" => proc.name.as_str(),
        program => program,
    };
    let mut words = vec![program];
    words.extend(proc.cmd.iter().skip(1).map(String::as_str));
    short_what(&words.join(" "))
}

/// Without quotes, backslashes and runs of white space: a shell's `-c` holds a command quoted.
fn plain_words(text: &str) -> String {
    text.chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What of a worker's processes is its work: each shell its CLI started, with what runs under
/// it, and what its commands left behind.
#[derive(Debug, Default)]
struct Work {
    shells: Vec<(u32, Vec<u32>)>,
    /// Each tree its commands left behind, its top pid first.
    left_behind: Vec<Vec<u32>>,
}

fn subtree(
    start: u32,
    marker: &str,
    procs: &HashMap<u32, &Proc>,
    children: &HashMap<u32, Vec<u32>>,
) -> Vec<u32> {
    let mut found = Vec::new();
    let mut stack = vec![start];
    while let Some(pid) = stack.pop() {
        // A session of another Brigadier run from here is its own (a dev app's workers).
        if procs
            .get(&pid)
            .and_then(|proc| proc.marker.as_deref())
            .is_some_and(|other| other != marker)
        {
            continue;
        }
        found.push(pid);
        if let Some(kids) = children.get(&pid) {
            stack.extend(kids);
        }
    }
    found
}

fn work_of(
    marker: &str,
    folder: Option<&Path>,
    daemon: u32,
    procs: &HashMap<u32, &Proc>,
    children: &HashMap<u32, Vec<u32>>,
) -> Work {
    let mut work = Work::default();
    let mut under: HashSet<u32> = HashSet::new();
    let clis: Vec<u32> = procs
        .values()
        .filter(|proc| proc.marker.as_deref() == Some(marker) && proc.parent == Some(daemon))
        .map(|proc| proc.pid)
        .collect();
    for cli in clis {
        under.extend(subtree(cli, marker, procs, children));
        let mut stack: Vec<u32> = children.get(&cli).cloned().unwrap_or_default();
        while let Some(pid) = stack.pop() {
            let Some(proc) = procs.get(&pid) else {
                continue;
            };
            if proc.marker.as_deref().is_some_and(|other| other != marker) {
                continue;
            }
            if is_named(proc, SHELLS) {
                work.shells
                    .push((pid, subtree(pid, marker, procs, children)));
            } else if !is_mcp(proc) {
                // A CLI may run through a launcher of its own (Codex: node, then its binary).
                if let Some(kids) = children.get(&pid) {
                    stack.extend(kids);
                }
            }
        }
    }
    let mut left: Vec<&Proc> = procs
        .values()
        .copied()
        .filter(|proc| {
            let marked = proc.marker.as_deref() == Some(marker) && proc.parent != Some(daemon);
            let hidden = || {
                matches!(proc.parent, None | Some(1))
                    && proc
                        .hidden_in
                        .as_ref()
                        .zip(folder)
                        .is_some_and(|(runs_in, folder)| Path::new(runs_in).starts_with(folder))
            };
            (marked || hidden()) && !under.contains(&proc.pid)
        })
        .collect();
    // A parent starts before its children, so the top of a tree comes first.
    left.sort_unstable_by_key(|proc| (proc.started_ms, proc.pid));
    for proc in left {
        if !work
            .left_behind
            .iter()
            .flatten()
            .any(|pid| *pid == proc.pid)
        {
            work.left_behind
                .push(subtree(proc.pid, marker, procs, children));
        }
    }
    work
}

fn is_compiler(proc: &Proc) -> bool {
    is_named(proc, COMPILERS)
}

/// A compile queued behind the slots. On Unix the launcher becomes the compiler, so one still
/// named so waits; on Windows it stays as the compiler's parent, so only one with no child does.
fn waits_for_slot(proc: &Proc, has_child: bool) -> bool {
    is_named(proc, &[LAUNCHER]) && (!cfg!(windows) || !has_child)
}

impl JobWatch {
    /// How often it looks: once a minute, more often when the limit is short.
    pub(crate) fn every(&self) -> Duration {
        Duration::from_millis(
            u64::try_from(self.after_ms / 4)
                .unwrap_or(0)
                .clamp(1_000, 60_000),
        )
    }

    /// Whether a look is due, and if so that it is taken now.
    pub(crate) fn due(&self) -> bool {
        let mut last = self
            .last_look
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() + Duration::from_millis(50) < self.every()) {
            return false;
        }
        *last = Some(Instant::now());
        true
    }

    /// Nothing is left to look at: the next look starts from nothing.
    pub(crate) fn idle(&self) {
        *self
            .clocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Clocks::default();
    }

    /// Looks at the machine for `workers` at `now`.
    pub(crate) fn look(&self, workers: &[SampledWorker], now: i64) -> Vec<Verdict> {
        let procs = {
            let mut system = self
                .system
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            read_processes(&mut system)
        };
        let outputs: HashMap<String, Output> = workers
            .iter()
            .flat_map(|worker| &worker.jobs)
            .filter_map(|job| {
                let meta = std::fs::metadata(job.output_file.as_deref()?).ok()?;
                Some((job.id.clone(), (meta.len(), meta.modified().ok())))
            })
            .collect();
        let machine = faked_machine().unwrap_or_else(|| Machine {
            load: sysinfo::System::load_average().one,
            cores: std::thread::available_parallelism()
                .map_or(1, |cores| u32::try_from(cores.get()).unwrap_or(u32::MAX)),
            faked: false,
        });
        // The machine says where a process runs with links resolved.
        let workers: Vec<SampledWorker> = workers
            .iter()
            .cloned()
            .map(|mut worker| {
                worker.folder = worker.folder.map(|folder| {
                    std::fs::canonicalize(&folder)
                        .map_or(folder, |real| real.to_string_lossy().into_owned())
                });
                worker
            })
            .collect();
        self.step(
            &workers,
            &procs,
            &outputs,
            machine,
            now,
            std::process::id(),
            &|file| super::cli_thread::last_line(Path::new(file)),
        )
    }

    /// One look: pure but for the clocks, so tests can tell it what the machine says.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn step(
        &self,
        workers: &[SampledWorker],
        procs: &[Proc],
        outputs: &HashMap<String, Output>,
        machine: Machine,
        now: i64,
        daemon: u32,
        last_line: &dyn Fn(&str) -> Option<String>,
    ) -> Vec<Verdict> {
        let mut clocks = self
            .clocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let every = i64::try_from(self.every().as_millis()).unwrap_or(i64::MAX);
        let elapsed = clocks.looked_ms.map(|at| now.saturating_sub(at));
        // No earlier look to measure from, or the machine slept between the two.
        let measured = elapsed.filter(|ms| *ms <= every.saturating_mul(3));
        if measured.is_none() {
            clocks.counted_from_ms = now;
            for clock in clocks.jobs.values_mut() {
                clock.since_ms = now;
            }
            for clock in clocks.left.values_mut() {
                clock.since_ms = now;
            }
            for clock in clocks.builds.values_mut() {
                clock.since_ms = now;
            }
        }
        let by_pid: HashMap<u32, &Proc> = procs.iter().map(|proc| (proc.pid, proc)).collect();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for proc in procs {
            if let Some(parent) = proc.parent {
                children.entry(parent).or_default().push(proc.pid);
            }
        }
        let moved_ms: HashMap<u32, u64> = procs
            .iter()
            .map(|proc| {
                let before = match (measured, clocks.cpu.get(&proc.pid)) {
                    (Some(_), Some(cpu)) => *cpu,
                    // New since the last look: all it did is news.
                    (Some(_), None) => 0,
                    (None, _) => proc.cpu_ms,
                };
                (proc.pid, proc.cpu_ms.saturating_sub(before))
            })
            .collect();
        let floor = measured.map_or(u64::MAX, |ms| {
            (ms as f64 * ALMOST_NO_CPU).ceil().max(1.0) as u64
        });
        let moved = |pids: &[u32]| -> u64 { pids.iter().filter_map(|pid| moved_ms.get(pid)).sum() };

        let mut verdicts = Vec::new();
        let mut live_left: HashSet<(TaskId, u32)> = HashSet::new();
        for worker in workers {
            let marker = worker.task.0.to_string();
            let folder = worker.folder.as_deref().map(Path::new);
            let work = work_of(&marker, folder, daemon, &by_pid, &children);
            let all: Vec<u32> = work
                .shells
                .iter()
                .flat_map(|(_, pids)| pids.iter().copied())
                .chain(work.left_behind.iter().flatten().copied())
                .collect();
            // Not an app left running, nor a server of a CLI that went.
            let left_trees: Vec<&Vec<u32>> = work
                .left_behind
                .iter()
                .filter(|pids| {
                    !pids.iter().any(|pid| {
                        by_pid
                            .get(pid)
                            .is_some_and(|proc| is_persistent_app(proc) || is_mcp(proc))
                    })
                })
                .collect();

            let build =
                self.build_hold(&all, &by_pid, &children, &moved_ms, measured, machine, now);
            let build_hold = build.map(|(kind, since, load)| {
                let clock = clocks
                    .builds
                    .entry(worker.task.clone())
                    .and_modify(|clock| {
                        if clock.kind != kind {
                            *clock = BuildClock {
                                kind,
                                since_ms: now,
                                told: false,
                            };
                        }
                    })
                    .or_insert(BuildClock {
                        kind,
                        since_ms: now,
                        told: false,
                    });
                (kind, since.unwrap_or(clock.since_ms), load)
            });
            if build_hold.is_none() {
                clocks.builds.remove(&worker.task);
            }
            let counted_from = clocks.counted_from_ms;
            let mut tell = Vec::new();
            let hold = build_hold.map(|(kind, since, load)| {
                let since = since.max(counted_from);
                let clock = clocks.builds.get_mut(&worker.task);
                if let Some(clock) = clock
                    && !clock.told
                    && now.saturating_sub(since) >= self.after_ms
                {
                    clock.told = true;
                    let time = minutes_words(now.saturating_sub(since));
                    let limit = minutes_words(self.after_ms);
                    tell.push((
                        EventKind::Starved,
                        match kind {
                            JobHoldKind::BuildSlot => format!(
                                "A compile has waited for a build slot for {time} (limit {limit}); not hung, others hold the slots."
                            ),
                            _ => format!(
                                "A compile has run far under one core for {time} (limit {limit}); load {} on {} cores{}.",
                                load.round() as u32,
                                machine.cores,
                                if machine.faked { ", faked" } else { "" }
                            ),
                        },
                    ));
                }
                let hold = JobHold {
                    kind,
                    since_ms: since,
                    what: None,
                    load: (kind == JobHoldKind::Starved).then(|| load.round() as u32),
                    cores: (kind == JobHoldKind::Starved).then_some(machine.cores),
                    faked: kind == JobHoldKind::Starved && machine.faked,
                };
                (None, hold)
            });

            // The shells each job runs in.
            let mut claimed: Vec<Vec<u32>> = vec![Vec::new(); worker.jobs.len()];
            for (at, job) in worker.jobs.iter().enumerate() {
                let Some(command) = job.command.as_deref().map(plain_words) else {
                    continue;
                };
                for (shell, pids) in &work.shells {
                    let Some(proc) = by_pid.get(shell) else {
                        continue;
                    };
                    if proc.started_ms.abs_diff(job.started_ms) <= 30_000
                        && plain_words(&proc.cmd.join(" ")).contains(&command)
                    {
                        claimed[at].extend(pids);
                    }
                }
            }

            let total = moved(&all);
            // When it began, the job it is if one, and what to call it.
            let mut stalled: Vec<(i64, Option<String>, String)> = Vec::new();
            let mut told_now: Vec<(&SampledJob, i64)> = Vec::new();
            for (at, job) in worker.jobs.iter().enumerate() {
                let pids = &claimed[at];
                let works = pids.iter().any(|pid| {
                    by_pid
                        .get(pid)
                        .is_some_and(|proc| !is_named(proc, SHELLS) && !is_named(proc, WAITERS))
                });
                let output = outputs.get(&job.id).copied();
                let clock = clocks
                    .jobs
                    .entry((worker.task.clone(), job.id.clone()))
                    .or_insert(JobClock {
                        since_ms: now,
                        output,
                        told: false,
                    });
                let held_build = hold.is_some()
                    && (!works
                        || pids.iter().any(|pid| {
                            by_pid.get(pid).is_some_and(|proc| {
                                is_compiler(proc) || is_named(proc, &[LAUNCHER])
                            })
                        }));
                // A job that only waits, beside what was left behind: that has its own clock,
                // and one stall is one wake.
                let moving = output != clock.output
                    || held_build
                    || (works && moved(pids) >= floor)
                    || (!works && (total >= floor || !left_trees.is_empty()));
                clock.output = output;
                if moving {
                    clock.since_ms = now;
                    clock.told = false;
                    continue;
                }
                let quiet = now.saturating_sub(clock.since_ms);
                if quiet >= self.after_ms && !clock.told {
                    clock.told = true;
                    told_now.push((job, quiet));
                }
                if clock.told {
                    stalled.push((clock.since_ms, Some(job.id.clone()), job.what.clone()));
                }
            }
            let mut left_told: Vec<(String, i64)> = Vec::new();
            for pids in &left_trees {
                let top = pids[0];
                let clock = clocks
                    .left
                    .entry((worker.task.clone(), top))
                    .or_insert(LeftClock {
                        since_ms: now,
                        told: false,
                    });
                let held_build = hold.is_some()
                    && pids.iter().any(|pid| {
                        by_pid
                            .get(pid)
                            .is_some_and(|proc| is_compiler(proc) || is_named(proc, &[LAUNCHER]))
                    });
                if held_build || moved(pids) >= floor {
                    clock.since_ms = now;
                    clock.told = false;
                    continue;
                }
                let quiet = now.saturating_sub(clock.since_ms);
                let what = by_pid
                    .get(&top)
                    .map_or_else(String::new, |p| command_words(p));
                if quiet >= self.after_ms && !clock.told {
                    clock.told = true;
                    left_told.push((what.clone(), quiet));
                }
                if clock.told {
                    stalled.push((clock.since_ms, None, what));
                }
            }
            live_left.extend(left_trees.iter().map(|pids| (worker.task.clone(), pids[0])));
            for (job, quiet) in told_now {
                let line = job
                    .output_file
                    .as_deref()
                    .and_then(last_line)
                    .map_or_else(|| "none".to_owned(), |line| short_what(&line));
                tell.push((
                    EventKind::Stalled,
                    format!(
                        "`{}`: no new output and almost no CPU for {} (limit {}); last output: {line}",
                        job.what,
                        minutes_words(quiet),
                        minutes_words(self.after_ms)
                    ),
                ));
            }
            for (what, quiet) in left_told {
                tell.push((
                    EventKind::Stalled,
                    format!(
                        "`{what}`, left behind by a command: almost no CPU for {} (limit {}); its output is not tracked.",
                        minutes_words(quiet),
                        minutes_words(self.after_ms)
                    ),
                ));
            }
            stalled.sort_by_key(|(since, _, _)| *since);
            let hold = hold.or_else(|| {
                stalled.first().map(|(since, job, what)| {
                    (
                        job.clone(),
                        JobHold {
                            kind: JobHoldKind::Stalled,
                            since_ms: *since,
                            what: Some(what.clone()),
                            load: None,
                            cores: None,
                            faked: false,
                        },
                    )
                })
            });
            verdicts.push(Verdict {
                task: worker.task.clone(),
                conversation: worker.conversation.clone(),
                hold,
                tell,
            });
        }
        // Forgets what no longer runs, so a job back under its id starts over.
        let live: HashSet<(TaskId, String)> = workers
            .iter()
            .flat_map(|worker| {
                worker
                    .jobs
                    .iter()
                    .map(|job| (worker.task.clone(), job.id.clone()))
            })
            .collect();
        clocks.jobs.retain(|key, _| live.contains(key));
        clocks.left.retain(|key, _| live_left.contains(key));
        let tasks: HashSet<&TaskId> = workers.iter().map(|worker| &worker.task).collect();
        clocks.builds.retain(|task, _| tasks.contains(task));
        clocks.cpu = procs.iter().map(|proc| (proc.pid, proc.cpu_ms)).collect();
        clocks.looked_ms = Some(now);
        verdicts
    }

    /// What holds up this worker's build now: its kind, since when if the machine says, and the
    /// load.
    #[allow(clippy::too_many_arguments)]
    fn build_hold(
        &self,
        all: &[u32],
        procs: &HashMap<u32, &Proc>,
        children: &HashMap<u32, Vec<u32>>,
        moved_ms: &HashMap<u32, u64>,
        measured: Option<i64>,
        machine: Machine,
        now: i64,
    ) -> Option<(JobHoldKind, Option<i64>, f64)> {
        let waiting = all
            .iter()
            .filter_map(|pid| procs.get(pid))
            .filter(|proc| waits_for_slot(proc, children.contains_key(&proc.pid)))
            .map(|proc| proc.started_ms)
            .min();
        if let Some(started) = waiting
            && now.saturating_sub(started) >= SLOT_SHOWN_AFTER_MS
        {
            return Some((JobHoldKind::BuildSlot, Some(started), machine.load));
        }
        let window = measured?;
        if machine.load <= f64::from(machine.cores) || window <= 0 {
            return None;
        }
        let busiest = all
            .iter()
            .filter_map(|pid| procs.get(pid))
            .filter(|proc| is_compiler(proc))
            .map(|proc| {
                let ran = window.min(now.saturating_sub(proc.started_ms)).max(1);
                moved_ms.get(&proc.pid).copied().unwrap_or(0) as f64 / ran as f64
            })
            .fold(None, |most: Option<f64>, rate| {
                Some(most.map_or(rate, |most| most.max(rate)))
            })?;
        (busiest < FAR_UNDER_ONE_CORE).then_some((JobHoldKind::Starved, None, machine.load))
    }
}

/// The machine reading a debug build was told to say instead of its own.
#[cfg(debug_assertions)]
fn faked_machine() -> Option<Machine> {
    parse_faked_machine(&std::env::var(FAKE_LOAD_ENV).ok()?)
}

/// A release build has no faked reading: it never looks at the environment for one.
#[cfg(not(debug_assertions))]
fn faked_machine() -> Option<Machine> {
    None
}

#[cfg(debug_assertions)]
fn parse_faked_machine(reading: &str) -> Option<Machine> {
    let (load, cores) = reading.split_once('/')?;
    Some(Machine {
        load: load.trim().parse().ok()?,
        cores: cores.trim().parse().ok()?,
        faked: true,
    })
}

fn read_processes(system: &mut sysinfo::System) -> Vec<Proc> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, UpdateKind};
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cpu()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    let prefix = format!("{}=", brigadier_cli::protocol::SESSION_ENV);
    system
        .processes()
        .values()
        // A thread of a process, as Linux lists them: its process counts it.
        .filter(|proc| proc.thread_kind().is_none())
        .map(|proc| Proc {
            pid: proc.pid().as_u32(),
            parent: proc.parent().map(sysinfo::Pid::as_u32),
            name: proc.name().to_string_lossy().into_owned(),
            cmd: proc
                .cmd()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
            marker: proc.environ().iter().find_map(|entry| {
                entry
                    .to_str()?
                    .strip_prefix(prefix.as_str())
                    .map(str::to_owned)
            }),
            hidden_in: proc
                .environ()
                .is_empty()
                .then(|| proc.cwd())
                .flatten()
                .map(|cwd| cwd.to_string_lossy().into_owned()),
            cpu_ms: proc.accumulated_cpu_time(),
            started_ms: i64::try_from(proc.start_time())
                .unwrap_or(0)
                .saturating_mul(1000),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAEMON: u32 = 1;
    const MIN: i64 = 60_000;
    const QUIET: Machine = Machine {
        load: 3.0,
        cores: 14,
        faked: false,
    };
    /// Faked: what a loaded machine says, for a starved build.
    const LOADED: Machine = Machine {
        load: 60.0,
        cores: 14,
        faked: false,
    };

    fn watch() -> JobWatch {
        JobWatch {
            after_ms: 10 * MIN,
            ..JobWatch::default()
        }
    }

    fn proc(pid: u32, parent: u32, name: &str, marked: bool, cpu_ms: u64) -> Proc {
        Proc {
            pid,
            parent: Some(parent),
            name: name.to_owned(),
            cmd: vec![name.to_owned()],
            marker: marked.then(|| "w".to_owned()),
            hidden_in: None,
            cpu_ms,
            started_ms: 0,
        }
    }

    /// A worker's CLI (2), its MCP server (3), and the shell (4) its job `what` runs in.
    fn tree(cli_cpu: u64, mcp_cpu: u64) -> Vec<Proc> {
        let mut shell = proc(4, 2, "zsh", false, 0);
        shell.cmd = vec!["/bin/zsh".into(), "-c".into(), "eval 'cargo test'".into()];
        let mut mcp = proc(3, 2, "server", true, mcp_cpu);
        mcp.cmd.push("mcp".into());
        vec![proc(2, DAEMON, "claude", true, cli_cpu), mcp, shell]
    }

    fn worker(jobs: &[(&str, Option<&str>)]) -> Vec<SampledWorker> {
        vec![SampledWorker {
            task: TaskId("w".into()),
            conversation: ConversationId("c".into()),
            jobs: jobs
                .iter()
                .map(|(id, file)| SampledJob {
                    id: (*id).to_owned(),
                    what: "cargo test".to_owned(),
                    command: Some("cargo test".to_owned()),
                    started_ms: 0,
                    output_file: file.map(str::to_owned),
                })
                .collect(),
            folder: None,
        }]
    }

    fn look(
        watch: &JobWatch,
        workers: &[SampledWorker],
        procs: &[Proc],
        machine: Machine,
        now: i64,
    ) -> Verdict {
        watch
            .step(
                workers,
                procs,
                &HashMap::new(),
                machine,
                now,
                DAEMON,
                &|_| Some("last line".to_owned()),
            )
            .remove(0)
    }

    fn with_child(child: Proc) -> Vec<Proc> {
        let mut procs = tree(0, 0);
        procs.push(child);
        procs
    }

    #[test]
    fn a_job_that_stops_moving_stalls_once_and_again_after_it_moved() {
        let watch = watch();
        let jobs = worker(&[("j", Some("out"))]);
        let cpu = |ms| with_child(proc(5, 4, "cargo", true, ms));
        let mut told = 0;
        for minute in 0..=13 {
            let verdict = look(&watch, &jobs, &cpu(0), QUIET, minute * MIN);
            told += verdict.tell.len();
            if minute == 12 {
                let (_, hold) = verdict.hold.clone().unwrap();
                assert_eq!(hold_words(&hold, minute * MIN), "Stalled 12m · cargo test");
            }
            if let Some((kind, message)) = verdict.tell.first() {
                assert_eq!(*kind, EventKind::Stalled);
                assert!(message.contains("`cargo test`"), "{message}");
                assert!(message.contains("limit 10 min"), "{message}");
                assert!(message.contains("last output: last line"), "{message}");
            }
        }
        assert_eq!(told, 1, "once per stall");
        // It moves: no longer stalled.
        let verdict = look(&watch, &jobs, &cpu(5_000), QUIET, 14 * MIN);
        assert_eq!(verdict.hold, None);
        // And stalls again: told again.
        let told_again: usize = (15..=25)
            .map(|minute| {
                look(&watch, &jobs, &cpu(5_000), QUIET, minute * MIN)
                    .tell
                    .len()
            })
            .sum();
        assert_eq!(told_again, 1);
    }

    #[test]
    fn a_silent_job_with_cpu_or_output_moving_never_stalls() {
        let watch = watch();
        let jobs = worker(&[("j", None)]);
        for minute in 0..=30 {
            let busy = with_child(proc(5, 4, "cargo", true, minute as u64 * 30_000));
            let verdict = look(&watch, &jobs, &busy, QUIET, minute * MIN);
            assert_eq!(verdict.hold, None);
            assert!(verdict.tell.is_empty());
        }
        let watch = super::tests::watch();
        let jobs = worker(&[("j", Some("out"))]);
        let idle = with_child(proc(5, 4, "cargo", true, 0));
        for minute in 0..=30i64 {
            let grows = HashMap::from([("j".to_owned(), (minute as u64, None))]);
            let verdict = watch
                .step(&jobs, &idle, &grows, QUIET, minute * MIN, DAEMON, &|_| None)
                .remove(0);
            assert_eq!(verdict.hold, None);
        }
    }

    /// The CLI and its MCP servers tick all the time: that hides no hang.
    #[test]
    fn the_cli_and_its_mcp_servers_count_for_nothing() {
        let watch = watch();
        let jobs = worker(&[("j", None)]);
        let mut told = Vec::new();
        for minute in 0..=11i64 {
            let ticks = minute as u64 * 30_000;
            let mut procs = tree(ticks, ticks);
            procs.push(proc(5, 4, "cargo", true, 0));
            told.extend(look(&watch, &jobs, &procs, QUIET, minute * MIN).tell);
        }
        assert_eq!(told.len(), 1, "{told:?}");
    }

    /// `( ... ) &`: the build lost its parent, and a tracked job only polls for it.
    #[test]
    fn a_polling_job_moves_with_what_a_command_left_behind() {
        let watch = watch();
        let jobs = worker(&[("j", None)]);
        let with_build = |cpu| {
            let mut procs = with_child(proc(5, 4, "sleep", false, 0));
            procs.push(proc(9, 0, "rustc", true, cpu));
            procs[3].parent = None;
            procs
        };
        for minute in 0..=12i64 {
            let verdict = look(
                &watch,
                &jobs,
                &with_build(minute as u64 * 30_000),
                QUIET,
                minute * MIN,
            );
            assert!(verdict.tell.is_empty(), "{:?}", verdict.tell);
        }
        // The build hangs: the watcher's polling is no progress.
        let told: usize = (13..=24)
            .map(|minute| {
                look(&watch, &jobs, &with_build(12 * 30_000), QUIET, minute * MIN)
                    .tell
                    .len()
            })
            .sum();
        assert_eq!(told, 1);
    }

    /// A watcher ticks, so its job moves; what it watches hangs. One event, the row says it.
    #[test]
    fn a_chatty_watcher_hides_no_hang_left_behind() {
        let watch = watch();
        let jobs = worker(&[("j", Some("out"))]);
        let procs = |cpu: u64| {
            let mut procs = with_child(proc(5, 4, "sleep", false, 0));
            let mut build = proc(9, 0, "cargo", true, cpu);
            build.parent = None;
            build.cmd = vec!["/u/bin/cargo".into(), "build".into()];
            procs.push(build);
            procs
        };
        let at = |minute: i64, cpu: u64| {
            let ticks = HashMap::from([("j".to_owned(), (minute as u64, None))]);
            watch
                .step(
                    &jobs,
                    &procs(cpu),
                    &ticks,
                    QUIET,
                    minute * MIN,
                    DAEMON,
                    &|_| None,
                )
                .remove(0)
        };
        let mut told = Vec::new();
        for minute in 0..=12 {
            let verdict = at(minute, 0);
            told.extend(verdict.tell);
            if minute == 12 {
                let (job, hold) = verdict.hold.unwrap();
                assert_eq!(job, None);
                assert_eq!(hold_words(&hold, minute * MIN), "Stalled 12m · cargo build");
            }
        }
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(told[0].0, EventKind::Stalled);
        assert!(
            told[0].1.starts_with("`cargo build`, left behind"),
            "{}",
            told[0].1
        );
        // It moves again, then hangs again: told again.
        assert_eq!(at(13, 5_000).hold, None);
        let again: usize = (14..=25).map(|minute| at(minute, 5_000).tell.len()).sum();
        assert_eq!(again, 1);
    }

    #[test]
    fn a_chatty_job_alone_stays_working() {
        let watch = watch();
        let jobs = worker(&[("j", Some("out"))]);
        let procs = with_child(proc(5, 4, "sleep", false, 0));
        for minute in 0..=30i64 {
            let ticks = HashMap::from([("j".to_owned(), (minute as u64, None))]);
            let verdict = watch
                .step(&jobs, &procs, &ticks, QUIET, minute * MIN, DAEMON, &|_| {
                    None
                })
                .remove(0);
            assert_eq!(verdict.hold, None);
            assert!(verdict.tell.is_empty(), "{:?}", verdict.tell);
        }
    }

    /// A busy tree left behind is a healthy build, however long it runs.
    #[test]
    fn a_busy_tree_left_behind_never_stalls() {
        let watch = watch();
        let jobs = worker(&[("j", Some("out"))]);
        for minute in 0..=30i64 {
            let mut build = proc(9, 0, "cargo", true, 0);
            build.parent = None;
            let rustc = proc(10, 9, "rustc", true, minute as u64 * 30_000);
            let procs = [
                with_child(proc(5, 4, "sleep", false, 0)),
                vec![build, rustc],
            ]
            .concat();
            let ticks = HashMap::from([("j".to_owned(), (minute as u64, None))]);
            let verdict = watch
                .step(&jobs, &procs, &ticks, QUIET, minute * MIN, DAEMON, &|_| {
                    None
                })
                .remove(0);
            assert_eq!(verdict.hold, None);
            assert!(verdict.tell.is_empty(), "{:?}", verdict.tell);
        }
    }

    #[test]
    fn an_idle_app_left_running_is_not_told() {
        for cmd in [
            vec!["/u/bin/vite", "--port", "1420"],
            vec![
                "/u/bin/node",
                "/r/node_modules/vite/bin/vite.js",
                "--port",
                "1420",
            ],
            vec!["/u/bin/node", "/u/bin/pnpm", "dev"],
            vec![
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "--headless",
            ],
        ] {
            let watch = watch();
            let jobs = worker(&[("j", Some("out"))]);
            let mut app = proc(9, 0, "node", true, 0);
            app.parent = None;
            app.cmd = cmd.iter().map(|word| (*word).to_owned()).collect();
            let procs = with_child(app);
            for minute in 0..=12i64 {
                let ticks = HashMap::from([("j".to_owned(), (minute as u64, None))]);
                let verdict = watch
                    .step(&jobs, &procs, &ticks, QUIET, minute * MIN, DAEMON, &|_| {
                        None
                    })
                    .remove(0);
                assert_eq!(verdict.hold, None, "{cmd:?}");
                assert!(verdict.tell.is_empty(), "{cmd:?}: {:?}", verdict.tell);
            }
        }
    }

    /// `( sleep 900 ) &` on macOS: an orphan with its environment hidden, known by its folder.
    #[test]
    fn an_orphan_with_a_hidden_environment_counts_in_the_workers_own_folder_only() {
        let mut jobs = worker(&[("j", Some("out"))]);
        jobs[0].folder = Some("/w/work".to_owned());
        let told_of = |runs_in: &str, parent: u32| {
            let watch = watch();
            let mut sleeper = proc(9, parent, "sleep", false, 0);
            sleeper.cmd = vec!["sleep".into(), "900".into()];
            sleeper.hidden_in = Some(runs_in.to_owned());
            let procs = with_child(sleeper);
            (0..=12i64)
                .flat_map(|minute| {
                    let ticks = HashMap::from([("j".to_owned(), (minute as u64, None))]);
                    watch
                        .step(&jobs, &procs, &ticks, QUIET, minute * MIN, DAEMON, &|_| {
                            None
                        })
                        .remove(0)
                        .tell
                })
                .collect::<Vec<_>>()
        };
        let told = told_of("/w/work/sub", 1);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            told[0].1.starts_with("`sleep 900`, left behind"),
            "{}",
            told[0].1
        );
        assert!(told_of("/w/work2", 1).is_empty());
        assert!(told_of("/w/work", 700).is_empty(), "has a live parent");
    }

    /// So no orchestrator takes a faked reading for the machine's.
    #[cfg(debug_assertions)]
    #[test]
    fn a_faked_machine_reading_says_so_on_the_row_and_in_the_event() {
        let machine = parse_faked_machine("60/14").unwrap();
        assert_eq!(
            (machine.load, machine.cores, machine.faked),
            (60.0, 14, true)
        );
        assert!(parse_faked_machine("sixty").is_none());

        let watch = watch();
        let jobs = worker(&[("j", None)]);
        let rustc = |minute: i64| with_child(proc(5, 4, "rustc", true, minute as u64 * 18_000));
        let mut tells = Vec::new();
        let mut last = None;
        for minute in 0..=12 {
            let verdict = look(&watch, &jobs, &rustc(minute), machine, minute * MIN);
            tells.extend(verdict.tell.clone());
            last = verdict.hold.or(last);
        }
        let (_, hold) = last.unwrap();
        assert_eq!(
            hold_words(&hold, 12 * MIN),
            "Build starved 11m · load 60 on 14 cores · faked"
        );
        assert_eq!(tells.len(), 1, "{tells:?}");
        assert!(
            tells[0].1.ends_with("load 60 on 14 cores, faked."),
            "{}",
            tells[0].1
        );
    }

    #[test]
    fn a_compile_queued_for_a_slot_says_so_at_once_and_wakes_once_never_stalled() {
        let watch = watch();
        let jobs = worker(&[("j", None)]);
        let mut queued = proc(5, 4, LAUNCHER, true, 0);
        queued.started_ms = 0;
        let procs = with_child(queued);
        let mut tells = Vec::new();
        for minute in 1..=12 {
            let verdict = look(&watch, &jobs, &procs, QUIET, minute * MIN);
            let (job, hold) = verdict.hold.clone().unwrap();
            assert_eq!(job, None);
            assert_eq!(hold.kind, JobHoldKind::BuildSlot);
            tells.extend(verdict.tell);
        }
        assert_eq!(tells.len(), 1, "{tells:?}");
        assert_eq!(tells[0].0, EventKind::Starved);
    }

    /// The load is faked here: nothing loads the machine.
    #[test]
    fn a_compile_far_under_one_core_on_a_loaded_machine_reads_starved() {
        let watch = watch();
        let jobs = worker(&[("j", None)]);
        let rustc = |minute: i64| with_child(proc(5, 4, "rustc", true, minute as u64 * 18_000));
        let mut tells = Vec::new();
        let mut last = None;
        for minute in 0..=12 {
            let verdict = look(&watch, &jobs, &rustc(minute), LOADED, minute * MIN);
            tells.extend(verdict.tell.clone());
            last = verdict.hold.or(last);
        }
        let (_, hold) = last.unwrap();
        assert_eq!(
            hold_words(&hold, 12 * MIN),
            "Build starved 11m · load 60 on 14 cores"
        );
        assert_eq!(tells.len(), 1, "{tells:?}");
        assert_eq!(tells[0].0, EventKind::Starved);
        // The same compile on an idle machine is no news, however slow.
        let watch = super::tests::watch();
        for minute in 0..=12 {
            assert_eq!(
                look(&watch, &jobs, &rustc(minute), QUIET, minute * MIN).hold,
                None
            );
        }
    }

    /// The Mac slept 40 minutes between two looks: none of that counts.
    #[test]
    fn time_asleep_counts_for_nothing() {
        let watch = watch();
        let jobs = worker(&[("j", None)]);
        let idle = with_child(proc(5, 4, "cargo", true, 0));
        for minute in 0..=8 {
            assert!(
                look(&watch, &jobs, &idle, QUIET, minute * MIN)
                    .tell
                    .is_empty()
            );
        }
        let woke = 48 * MIN;
        let verdict = look(&watch, &jobs, &idle, QUIET, woke);
        assert_eq!(verdict.hold, None);
        assert!(verdict.tell.is_empty());
        // A whole limit after waking, it stalls.
        let told: usize = (1..=10)
            .map(|minute| {
                look(&watch, &jobs, &idle, QUIET, woke + minute * MIN)
                    .tell
                    .len()
            })
            .sum();
        assert_eq!(told, 1);
    }
}
