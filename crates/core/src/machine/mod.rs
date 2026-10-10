//! The machine guard: Brigadier holds back its own heavy work while the machine struggles,
//! automatically and with no setting (PLAN.md §10.7).
//!
//! - Serious heat or critical memory pressure holds new workers, builds and test runs.
//!   Memory warnings hold only new builds and tests, for at most two minutes before a free
//!   build lease may proceed. They wait with a grey row in the thread
//!   and start when it eases. Work already running is left alone.
//! - Heavy commands run one at a time daemon-wide (the build lease, see [`builds`]).
//! - Critical heat held for a minute pauses Brigadier's own running builds, newest first,
//!   until it drops back; nothing is ever killed. The user's own apps and terminals, and the
//!   worker CLIs themselves, are never touched.
//! - Whatever is stopped is written down before it is stopped, so a daemon that quits or
//!   crashes lets it go on: at quit, or at the next start.

pub(crate) mod builds;
pub(crate) mod heavy;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use brigadier_sandbox::{Heat, MachineLoad, MemoryPressure, Platform};
use tokio::sync::watch;

use self::builds::{Action, Builds, Note, Proc, Seen};

/// How often the watch looks at the machine and at Brigadier's process trees.
pub(crate) const TICK: Duration = Duration::from_secs(2);
/// CPU time a command's processes must use between two looks to count as working: 5% of
/// one core.
const ACTIVE_CPU_MS: u64 = 100;
/// How long something waiting for the machine sleeps between looks at whether it is still
/// wanted.
pub(crate) const RECHECK: Duration = Duration::from_secs(20);

/// What a thread row says about a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Row {
    /// The command's CLI's owner in the cleanup ledger.
    pub owner: String,
    pub command: String,
    pub note: Note,
}

/// The guard, the build lease and what is stopped, together.
pub(crate) struct MachineWatch {
    platform: Arc<dyn Platform>,
    pub(crate) guard: MachineGuard,
    builds: Mutex<Builds>,
    stopped: Stopped,
    quit: AtomicBool,
    /// The CPU time each process of a heavy command had used at the last look, in ms.
    cpu: Mutex<HashMap<u32, u64>>,
    /// When a command of each owner was last held, in ms since the epoch.
    held: Mutex<HashMap<String, i64>>,
}

impl MachineWatch {
    /// `stopped_file` is where what is stopped is written down.
    pub(crate) fn new(platform: Arc<dyn Platform>, stopped_file: PathBuf) -> Self {
        Self {
            guard: MachineGuard::new(platform.clone()),
            platform,
            builds: Mutex::new(Builds::default()),
            stopped: Stopped::new(stopped_file),
            quit: AtomicBool::new(false),
            cpu: Mutex::new(HashMap::new()),
            held: Mutex::new(HashMap::new()),
        }
    }

    /// At start: lets go on whatever an earlier daemon left stopped. Blocking.
    pub(crate) fn recover(&self) -> usize {
        self.stopped.sweep(&*self.platform)
    }

    /// One look at `now`: reads the machine, finds the heavy commands under `clis` (each
    /// Brigadier CLI process with its owner), stops and lets go on what the lease and the heat
    /// say. Returns what the threads should say. Blocking (it walks process trees).
    pub(crate) fn tick(&self, clis: &[(String, Proc)], now: Instant) -> Vec<Row> {
        let load = self.guard.read();
        let platform = &*self.platform;
        let mut seen: Vec<Seen> = clis
            .iter()
            .filter(|(_, cli)| still(platform, *cli))
            .flat_map(|(owner, cli)| heavy_under(platform, cli.pid, owner))
            .collect();
        seen = outermost(platform, seen);
        self.mark_active(&mut seen);
        // Held while acting too, so quitting waits for a round under way and none acts after.
        let mut builds = self.builds.lock().unwrap_or_else(|p| p.into_inner());
        if self.quit.load(Ordering::Acquire) {
            return Vec::new();
        }
        let mut rows = Vec::new();
        let mut unstopped = Vec::new();
        for action in builds.tick(seen, load, now) {
            match action {
                Action::Stop(proc) => {
                    if !self.stopped.stop(platform, proc) {
                        unstopped.push(proc);
                    }
                }
                Action::Continue(proc) => self.stopped.resume(platform, proc),
                Action::Note {
                    proc,
                    owner,
                    command,
                    note,
                } => {
                    if !unstopped.contains(&proc) {
                        rows.push(Row {
                            owner,
                            command,
                            note,
                        });
                    }
                }
            }
        }
        for proc in unstopped {
            builds.left_running(proc);
        }
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let stamp = crate::now_ms();
        held.retain(|_, at| stamp - *at < 24 * 60 * 60 * 1000);
        for owner in builds.held_owners() {
            held.insert(owner, stamp);
        }
        rows
    }

    /// When a command of `owner` was last held (waiting or paused), in ms since the epoch:
    /// time its worker spent silent for the machine, which the stall watchdog doesn't count.
    pub(crate) fn held_at_ms(&self, owner: &str) -> Option<i64> {
        self.held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(owner)
            .copied()
    }

    /// Marks each command whose processes used CPU since the last look, or started since.
    fn mark_active(&self, seen: &mut [Seen]) {
        let processes = self.platform.processes();
        let mut cpu = self.cpu.lock().unwrap_or_else(|p| p.into_inner());
        let mut now = HashMap::new();
        for command in seen.iter_mut() {
            let mut used = 0;
            let mut started = false;
            let tree = std::iter::once(command.root.pid)
                .chain(processes.descendants(command.root.pid).unwrap_or_default());
            for pid in tree {
                let Some(ms) = processes.cpu_time_ms(pid) else {
                    continue;
                };
                match cpu.get(&pid) {
                    Some(before) => used += ms.saturating_sub(*before),
                    None => started = true,
                }
                now.insert(pid, ms);
            }
            command.active = started || used >= ACTIVE_CPU_MS;
        }
        *cpu = now;
    }

    /// The daemon quits: everything stopped goes on, and nothing is stopped after. Blocking.
    pub(crate) fn quit(&self) {
        let mut builds = self.builds.lock().unwrap_or_else(|p| p.into_inner());
        self.quit.store(true, Ordering::Release);
        for action in builds.release_all() {
            if let Action::Continue(proc) = action {
                self.stopped.resume(&*self.platform, proc);
            }
        }
        self.stopped.sweep(&*self.platform);
    }

    /// Waits until workers can start, at most `max`; whether the machine eased.
    pub(crate) async fn eased_within(&self, max: Duration) -> bool {
        let mut changes = self.guard.subscribe();
        let _ = tokio::time::timeout(max, changes.wait_for(|load| !load.workers_held())).await;
        !self.guard.current().workers_held()
    }
}

/// Where the guard reads the machine's load from.
pub(crate) struct MachineGuard {
    platform: Arc<dyn Platform>,
    /// A load set in place of the OS's (tests).
    fake: Mutex<Option<MachineLoad>>,
    /// A development build's stand-in for the OS: a file naming the load (`calm`, `hot`,
    /// `memory` (warning), `memory-critical`, `critical`), from `BRIGADIER_FAKE_MACHINE`,
    /// so the guard can be tried live.
    fake_file: Option<PathBuf>,
    load: watch::Sender<MachineLoad>,
}

impl MachineGuard {
    pub(crate) fn new(platform: Arc<dyn Platform>) -> Self {
        let fake_file = cfg!(debug_assertions)
            .then(|| std::env::var_os("BRIGADIER_FAKE_MACHINE"))
            .flatten()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        Self {
            platform,
            fake: Mutex::new(None),
            fake_file,
            load: watch::Sender::new(MachineLoad::default()),
        }
    }

    /// Reads the load now and tells whoever waits on it when it changed.
    pub(crate) fn read(&self) -> MachineLoad {
        let fake = *self.fake.lock().unwrap_or_else(|p| p.into_inner());
        let load = match (fake, &self.fake_file) {
            (Some(load), _) => load,
            (None, Some(file)) => std::fs::read_to_string(file)
                .map(|text| parse_fake(&text))
                .unwrap_or_default(),
            (None, None) => self.platform.machine().load(),
        };
        self.load.send_if_modified(|known| {
            let changed = *known != load;
            *known = load;
            changed
        });
        load
    }

    /// The load as last read.
    pub(crate) fn current(&self) -> MachineLoad {
        *self.load.borrow()
    }

    /// Changes of the load, as they are read.
    pub(crate) fn subscribe(&self) -> watch::Receiver<MachineLoad> {
        self.load.subscribe()
    }

    /// Stands in for the OS from now on (tests).
    #[cfg(test)]
    pub(crate) fn fake(&self, load: MachineLoad) {
        *self.fake.lock().unwrap_or_else(|p| p.into_inner()) = Some(load);
        self.read();
    }
}

fn parse_fake(text: &str) -> MachineLoad {
    match text.trim() {
        "hot" => MachineLoad {
            heat: Heat::Serious,
            memory: MemoryPressure::Normal,
        },
        "memory" => MachineLoad {
            heat: Heat::Nominal,
            memory: MemoryPressure::Warning,
        },
        "memory-critical" => MachineLoad {
            heat: Heat::Nominal,
            memory: MemoryPressure::Critical,
        },
        "critical" => MachineLoad {
            heat: Heat::Critical,
            memory: MemoryPressure::Normal,
        },
        _ => MachineLoad::default(),
    }
}

/// `pid` as a [`Proc`], if it runs.
pub(crate) fn proc_of(platform: &dyn Platform, pid: u32) -> Option<Proc> {
    let started = platform.processes().start_time_ms(pid).ok()?;
    Some(Proc {
        pid,
        started_ms: started.round() as i64,
    })
}

/// Whether `proc` still is the process it was (alive, the same start time).
pub(crate) fn still(platform: &dyn Platform, proc: Proc) -> bool {
    platform.processes().is_alive(proc.pid)
        && proc_of(platform, proc.pid)
            .is_some_and(|now| (now.started_ms - proc.started_ms).abs() < 1_000)
}

/// The heavy commands running under `cli` (a CLI process Brigadier started), the topmost
/// heavy process of each branch. A CLI never counts, but the ledger also tracks the group
/// leaders of the commands a CLI runs: one that is itself heavy is the command, and what it
/// starts (each `rustc` of a `cargo build`) belongs to it.
pub(crate) fn heavy_under(platform: &dyn Platform, cli: u32, owner: &str) -> Vec<Seen> {
    let processes = platform.processes();
    if let Some(argv) = processes.command_line(cli)
        && heavy::is_heavy(&argv)
    {
        return proc_of(platform, cli)
            .map(|root| Seen {
                root,
                owner: owner.to_owned(),
                command: heavy::label(&argv),
                active: true,
            })
            .into_iter()
            .collect();
    }
    let mut found = Vec::new();
    let mut queue = processes.children(cli).unwrap_or_default();
    let mut visited = 0;
    while let Some(pid) = queue.pop() {
        // A runaway fork chain can't keep the walk going forever.
        visited += 1;
        if visited > 2_000 {
            break;
        }
        let Some(argv) = processes.command_line(pid) else {
            continue;
        };
        if heavy::is_heavy(&argv) {
            if let Some(root) = proc_of(platform, pid) {
                found.push(Seen {
                    root,
                    owner: owner.to_owned(),
                    command: heavy::label(&argv),
                    active: true,
                });
            }
            continue;
        }
        queue.extend(processes.children(pid).unwrap_or_default());
    }
    found
}

/// One command per process tree. The ledger tracks the group leaders of the commands a CLI
/// runs, so a heavy command started inside another one in a process group of its own
/// (`pnpm test` running `pnpm --filter app test`) is also seen on its own: it belongs to the
/// outer command, and must never wait for the lease its own parent holds.
pub(crate) fn outermost(platform: &dyn Platform, seen: Vec<Seen>) -> Vec<Seen> {
    let processes = platform.processes();
    let mut inside = std::collections::HashSet::new();
    for command in &seen {
        let mut queue = processes.children(command.root.pid).unwrap_or_default();
        while let Some(pid) = queue.pop() {
            if inside.len() > 10_000 || !inside.insert(pid) {
                continue;
            }
            queue.extend(processes.children(pid).unwrap_or_default());
        }
    }
    seen.into_iter()
        .filter(|command| !inside.contains(&command.root.pid))
        .collect()
}

/// Stops `root` and everything below it, parents before their children so nothing forks past
/// the walk. Returns every process stopped.
pub(crate) fn stop_tree(platform: &dyn Platform, root: Proc) -> Vec<Proc> {
    let processes = platform.processes();
    if !still(platform, root) {
        return Vec::new();
    }
    let mut stopped = Vec::new();
    let mut queue = vec![root.pid];
    while let Some(pid) = queue.pop() {
        if stopped.iter().any(|proc: &Proc| proc.pid == pid) || stopped.len() > 2_000 {
            continue;
        }
        let Some(proc) = proc_of(platform, pid) else {
            continue;
        };
        if processes.suspend(pid).is_ok() {
            stopped.push(proc);
        }
        queue.extend(processes.children(pid).unwrap_or_default());
    }
    stopped
}

/// Lets go on what [`stop_tree`] stopped, and anything still below `root`, children before
/// their parents.
pub(crate) fn continue_tree(platform: &dyn Platform, root: Proc, members: &[Proc]) {
    let processes = platform.processes();
    let mut procs: Vec<Proc> = members.to_vec();
    if still(platform, root) {
        for pid in processes.descendants(root.pid).unwrap_or_default() {
            if !procs.iter().any(|proc| proc.pid == pid)
                && let Some(proc) = proc_of(platform, pid)
            {
                procs.push(proc);
            }
        }
        if !procs.contains(&root) {
            procs.insert(0, root);
        }
    }
    for proc in procs.into_iter().rev() {
        if still(platform, proc)
            && let Err(err) = processes.resume(proc.pid)
        {
            tracing::warn!(pid = proc.pid, error = %err, "could not let a stopped process go on");
        }
    }
}

/// What Brigadier stopped and hasn't let go on yet, by command, written down before anything
/// is stopped: a daemon that dies lets it go on at its next start ([`Stopped::sweep`]).
pub(crate) struct Stopped {
    path: PathBuf,
    procs: Mutex<HashMap<Proc, Vec<Proc>>>,
}

impl Stopped {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            procs: Mutex::new(HashMap::new()),
        }
    }

    fn procs(&self) -> std::sync::MutexGuard<'_, HashMap<Proc, Vec<Proc>>> {
        self.procs.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Stops `root`'s tree, written down first (the root, then every member stopped).
    /// Nothing is stopped when it can't be written down (a full disk): whether it was.
    pub(crate) fn stop(&self, platform: &dyn Platform, root: Proc) -> bool {
        {
            let mut procs = self.procs();
            procs.entry(root).or_default();
            if let Err(err) = self.save(&procs) {
                tracing::warn!(error = %err, "left a build running: the stopped processes can't be written down");
                procs.remove(&root);
                return false;
            }
        }
        let members = stop_tree(platform, root);
        let mut procs = self.procs();
        procs.insert(root, members);
        // The root is written down already: a next daemon finds the rest below it.
        if let Err(err) = self.save(&procs) {
            tracing::warn!(error = %err, "could not write down a stopped build's processes");
        }
        true
    }

    /// Lets `root`'s tree go on and crosses it off.
    pub(crate) fn resume(&self, platform: &dyn Platform, root: Proc) {
        let members = self.procs().get(&root).cloned().unwrap_or_default();
        continue_tree(platform, root, &members);
        let mut procs = self.procs();
        procs.remove(&root);
        if let Err(err) = self.save(&procs) {
            tracing::warn!(error = %err, "could not write down the stopped processes");
        }
    }

    /// Lets everything written down go on: what a daemon that died left stopped (at start), or
    /// what this one stopped (at quit).
    pub(crate) fn sweep(&self, platform: &dyn Platform) -> usize {
        let mut procs = std::mem::take(&mut *self.procs());
        if let Ok(text) = std::fs::read_to_string(&self.path) {
            match serde_json::from_str::<Vec<(Proc, Vec<Proc>)>>(&text) {
                Ok(saved) => {
                    for (root, members) in saved {
                        procs.entry(root).or_default().extend(members);
                    }
                }
                Err(err) => tracing::warn!(error = %err, "unreadable list of stopped processes"),
            }
        }
        let count = procs.len();
        for (root, members) in procs {
            continue_tree(platform, root, &members);
        }
        let _ = std::fs::remove_file(&self.path);
        count
    }

    fn save(&self, procs: &HashMap<Proc, Vec<Proc>>) -> std::io::Result<()> {
        if procs.is_empty() {
            return match std::fs::remove_file(&self.path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            };
        }
        let list: Vec<(&Proc, &Vec<Proc>)> = procs.iter().collect();
        serde_json::to_vec(&list)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_atomically(&self.path, &bytes))
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temp = path.with_extension("json.new");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Child, Command};

    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("brigadier-machine-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn platform(dir: &Path) -> Arc<dyn Platform> {
        brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.to_path_buf()),
        })
        .unwrap()
    }

    fn state(pid: u32) -> String {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A stand-in worker CLI (`sh`) running a stand-in build: a program named `cargo` (a copy
    /// of `sleep`) run as `cargo test`, which starts a child of its own. Returns once the build
    /// has started that child, so no test stops it halfway through starting.
    fn worker_with_build(dir: &Path) -> (Child, PathBuf) {
        worker_with_build_running(dir, "/bin/sleep 300")
    }

    /// [`worker_with_build`] whose build's child runs `child` (a shell command line).
    fn worker_with_build_running(dir: &Path, child: &str) -> (Child, PathBuf) {
        let bin = dir.join("bin");
        std::fs::create_dir_all(dir).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        let cargo = bin.join("cargo");
        let ready = dir.join("ready");
        std::fs::write(
            &cargo,
            format!("#!/bin/sh\n{child} &\ntouch '{}'\nwait\n", ready.display()),
        )
        .unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let child = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("{} test -p core; true", cargo.display()))
            .spawn()
            .unwrap();
        wait_for(|| ready.exists().then_some(()));
        (child, cargo)
    }

    /// How long a process the test started may take to get where it waits for. Only a hang
    /// takes this long: a process starting, stopping or going on is quick on a calm machine,
    /// but each step is many times slower on one loaded far past its cores.
    const PATIENCE: Duration = Duration::from_secs(300);

    fn wait_for<T>(mut found: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(value) = found() {
                return value;
            }
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn a_build_under_a_worker_is_found_stopped_and_let_go_on() {
        let dir = TempDir::new();
        let platform = platform(dir.path());
        let (mut worker, _) = worker_with_build(dir.path());
        let seen = wait_for(|| {
            let seen = heavy_under(&*platform, worker.id(), "task:t");
            // The build and its own child are up.
            (seen.len() == 1
                && platform
                    .processes()
                    .children(seen[0].root.pid)
                    .is_ok_and(|children| !children.is_empty()))
            .then_some(seen)
        });
        assert_eq!(seen[0].owner, "task:t");
        assert_eq!(seen[0].command, "cargo test -p core");
        let root = seen[0].root;
        let list = Stopped::new(dir.path().join("stopped.json"));
        list.stop(&*platform, root);
        let members = list.procs().get(&root).cloned().unwrap();
        assert!(members.len() >= 2, "the build and its child: {members:?}");
        for member in &members {
            assert!(stopped(member.pid), "stopped, not killed");
        }
        assert!(dir.path().join("stopped.json").exists());
        list.resume(&*platform, root);
        for member in &members {
            assert!(running(member.pid), "running again: {}", state(member.pid));
        }
        assert!(!dir.path().join("stopped.json").exists());
        platform.processes().kill_tree(worker.id()).unwrap();
        worker.wait().unwrap();
    }

    #[test]
    fn a_tracked_build_is_one_command_not_one_per_compiler() {
        // The ledger tracks a command's group leader next to its CLI. Looked at on its own, a
        // build is still the command: the compilers it starts don't each wait for the lease.
        let dir = TempDir::new();
        let platform = platform(dir.path());
        let rustc = dir.path().join("rustc");
        std::fs::write(&rustc, "#!/bin/sh\n/bin/sleep 300\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&rustc, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (mut worker, _) = worker_with_build_running(dir.path(), &rustc.display().to_string());
        let build = wait_for(|| {
            let seen = heavy_under(&*platform, worker.id(), "task:t");
            (seen.len() == 1
                && platform
                    .processes()
                    .children(seen[0].root.pid)
                    .is_ok_and(|children| !children.is_empty()))
            .then(|| seen[0].root)
        });
        let seen = heavy_under(&*platform, build.pid, "task:t");
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert_eq!(seen[0].root, build);
        assert_eq!(seen[0].command, "cargo test -p core");
        platform.processes().kill_tree(worker.id()).unwrap();
        worker.wait().unwrap();
    }

    #[test]
    fn a_build_inside_a_build_is_part_of_it() {
        // `pnpm test` runs `pnpm --filter app test` in a process group of its own, which the
        // ledger tracks next to the CLI: it is seen on its own too, but it is the outer
        // command's, not a second build waiting for the lease its parent holds.
        let dir = TempDir::new();
        let platform = platform(dir.path());
        let inner = dir.path().join("inner").join("cargo");
        std::fs::create_dir_all(inner.parent().unwrap()).unwrap();
        std::fs::write(&inner, "#!/bin/sh\n/bin/sleep 300\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let (mut worker, _) =
            worker_with_build_running(dir.path(), &format!("{} build", inner.display()));
        let outer = wait_for(|| heavy_under(&*platform, worker.id(), "task:t").pop());
        let nested = wait_for(|| {
            let mut queue = platform.processes().children(outer.root.pid).ok()?;
            while let Some(pid) = queue.pop() {
                let seen = heavy_under(&*platform, pid, "task:t");
                if let Some(seen) = seen.into_iter().find(|seen| seen.root.pid == pid) {
                    return Some(seen);
                }
                queue.extend(platform.processes().children(pid).unwrap_or_default());
            }
            None
        });
        assert_eq!(nested.command, "cargo build");
        let seen = outermost(&*platform, vec![nested, outer.clone()]);
        assert_eq!(seen, vec![outer], "only the outer command");
        platform.processes().kill_tree(worker.id()).unwrap();
        worker.wait().unwrap();
    }

    #[test]
    fn a_restart_lets_go_on_what_a_dead_daemon_left_stopped() {
        let dir = TempDir::new();
        let platform = platform(dir.path());
        let (mut worker, _) = worker_with_build(dir.path());
        let root = wait_for(|| {
            heavy_under(&*platform, worker.id(), "task:t")
                .first()
                .map(|seen| seen.root)
        });
        let file = dir.path().join("stopped.json");
        // The daemon that stopped it dies without letting it go on.
        {
            let list = Stopped::new(file.clone());
            list.stop(&*platform, root);
            assert!(stopped(root.pid));
        }
        // The next one finds it written down.
        let next = Stopped::new(file.clone());
        assert_eq!(next.sweep(&*platform), 1);
        assert!(running(root.pid));
        assert!(!file.exists());
        platform.processes().kill_tree(worker.id()).unwrap();
        worker.wait().unwrap();
    }

    const HOT: MachineLoad = MachineLoad {
        heat: Heat::Serious,
        memory: MemoryPressure::Normal,
    };
    const CRITICAL: MachineLoad = MachineLoad {
        heat: Heat::Critical,
        memory: MemoryPressure::Normal,
    };

    /// Whether `pid` shows as stopped: a stop lands when the process is next scheduled, which a
    /// loaded machine can put a while after the signal.
    fn stopped(pid: u32) -> bool {
        settles(pid, |state| state.starts_with('T'))
    }

    /// Whether `pid` shows as alive and not stopped.
    fn running(pid: u32) -> bool {
        settles(pid, |state| !state.is_empty() && !state.starts_with('T'))
    }

    fn settles(pid: u32, ok: impl Fn(&str) -> bool) -> bool {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if ok(&state(pid)) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn end(platform: &dyn Platform, mut worker: Child) {
        platform.processes().kill_tree(worker.id()).unwrap();
        worker.wait().unwrap();
    }

    /// Done-when (0): with the guard faked to hot, a new worker and a new build wait (a row
    /// says so) and start when it clears; running work is untouched.
    #[tokio::test]
    async fn while_hot_new_workers_and_builds_wait_and_running_work_is_untouched() {
        let dir = TempDir::new();
        let platform = platform(dir.path());
        let watch = Arc::new(MachineWatch::new(
            platform.clone(),
            dir.path().join("stopped.json"),
        ));
        // Calm, whatever the host's heat or memory pressure now.
        watch.guard.fake(MachineLoad::default());
        let t0 = Instant::now();
        // A build runs while the machine is calm.
        let (first, _) = worker_with_build(&dir.path().join("a"));
        let first_cli = proc_of(&*platform, first.id()).unwrap();
        let calm = wait_for(|| {
            heavy_under(&*platform, first.id(), "task:a")
                .first()
                .map(|seen| seen.root)
        });
        let mut clis = vec![("task:a".to_owned(), first_cli)];
        assert!(watch.tick(&clis, t0).is_empty());
        assert!(watch.eased_within(Duration::ZERO).await);

        watch.guard.fake(HOT);
        // A new worker waits…
        assert!(!watch.eased_within(Duration::from_millis(100)).await);
        let worker = tokio::spawn({
            let watch = watch.clone();
            async move { watch.eased_within(Duration::from_secs(30)).await }
        });
        // …and so does a new build, with a row; the running one is left alone.
        let (second, _) = worker_with_build(&dir.path().join("b"));
        let second_cli = proc_of(&*platform, second.id()).unwrap();
        let waiting = wait_for(|| {
            heavy_under(&*platform, second.id(), "task:b")
                .first()
                .map(|seen| seen.root)
        });
        clis.push(("task:b".to_owned(), second_cli));
        let rows = watch.tick(&clis, t0 + TICK);
        assert_eq!(
            rows,
            vec![Row {
                owner: "task:b".into(),
                command: "cargo test -p core".into(),
                note: Note::WaitingToCool(crate::model::MachineStepReason::Heat),
            }]
        );
        assert!(stopped(waiting.pid));
        assert!(running(calm.pid), "running work untouched");
        // The running build ends; still hot, so the new one keeps waiting.
        end(&*platform, first);
        clis.remove(0);
        assert!(watch.tick(&clis, t0 + TICK * 2).is_empty());
        assert!(stopped(waiting.pid));
        assert!(!worker.is_finished());

        // It clears: the worker starts, and the build goes on.
        watch.guard.fake(MachineLoad::default());
        assert!(worker.await.unwrap());
        watch.tick(&clis, t0 + TICK * 3);
        assert!(running(waiting.pid));
        assert!(!dir.path().join("stopped.json").exists());
        end(&*platform, second);
    }

    /// Done-when (0b): critical heat for a minute suspends a running build (not killed), it
    /// goes on when the heat drops, and a restarted daemon lets go on what was left stopped.
    #[tokio::test]
    async fn critical_heat_for_a_minute_pauses_a_build_until_it_cools_and_a_restart_resumes_it() {
        let dir = TempDir::new();
        let platform = platform(dir.path());
        let file = dir.path().join("stopped.json");
        let watch = MachineWatch::new(platform.clone(), file.clone());
        // Calm, whatever the host's heat or memory pressure now.
        watch.guard.fake(MachineLoad::default());
        let t0 = Instant::now();
        let (worker, _) = worker_with_build(dir.path());
        let cli = proc_of(&*platform, worker.id()).unwrap();
        let build = wait_for(|| {
            heavy_under(&*platform, worker.id(), "task:a")
                .first()
                .map(|seen| seen.root)
        });
        let clis = vec![("task:a".to_owned(), cli)];
        watch.tick(&clis, t0);

        watch.guard.fake(CRITICAL);
        assert!(watch.tick(&clis, t0 + Duration::from_secs(1)).is_empty());
        assert!(watch.tick(&clis, t0 + Duration::from_secs(40)).is_empty());
        assert!(running(build.pid));
        let rows = watch.tick(&clis, t0 + Duration::from_secs(62));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].note, Note::Paused);
        assert!(stopped(build.pid), "suspended");
        assert!(platform.processes().is_alive(build.pid), "not killed");
        assert!(file.exists(), "written down before it was stopped");

        // Back to serious: it goes on.
        watch.guard.fake(HOT);
        let rows = watch.tick(&clis, t0 + Duration::from_secs(64));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].note, Note::Resumed);
        assert_eq!(rows[0].command, "cargo test -p core");
        assert!(running(build.pid));

        // Critical again for a minute, and the daemon dies with the build paused.
        watch.guard.fake(CRITICAL);
        watch.tick(&clis, t0 + Duration::from_secs(70));
        watch.tick(&clis, t0 + Duration::from_secs(131));
        assert!(stopped(build.pid));
        drop(watch);
        assert!(stopped(build.pid));
        // The next daemon lets it go on as it starts.
        let next = MachineWatch::new(platform.clone(), file.clone());
        assert_eq!(next.recover(), 1);
        assert!(running(build.pid));
        assert!(!file.exists());
        end(&*platform, worker);
    }

    #[tokio::test]
    async fn quitting_lets_stopped_builds_go_on() {
        let dir = TempDir::new();
        let platform = platform(dir.path());
        let watch = MachineWatch::new(platform.clone(), dir.path().join("stopped.json"));
        watch.guard.fake(HOT);
        let (worker, _) = worker_with_build(dir.path());
        let cli = proc_of(&*platform, worker.id()).unwrap();
        let build = wait_for(|| {
            heavy_under(&*platform, worker.id(), "task:a")
                .first()
                .map(|seen| seen.root)
        });
        watch.tick(&[("task:a".to_owned(), cli)], Instant::now());
        assert!(stopped(build.pid));
        watch.quit();
        assert!(running(build.pid));
        end(&*platform, worker);
    }

    #[tokio::test]
    async fn memory_warning_allows_workers_but_critical_holds_them() {
        let dir = TempDir::new();
        let watch = MachineWatch::new(platform(dir.path()), dir.path().join("stopped.json"));
        watch.guard.fake(parse_fake("memory"));
        assert!(watch.eased_within(Duration::ZERO).await);
        watch.guard.fake(parse_fake("memory-critical"));
        assert!(!watch.eased_within(Duration::ZERO).await);
        watch.guard.fake(parse_fake("hot"));
        assert!(!watch.eased_within(Duration::ZERO).await);
        watch.guard.fake(parse_fake("calm"));
        assert!(watch.eased_within(Duration::ZERO).await);
    }

    #[test]
    fn a_fake_load_stands_in_for_the_os() {
        let dir = TempDir::new();
        let guard = MachineGuard::new(platform(dir.path()));
        let changes = guard.subscribe();
        guard.fake(MachineLoad {
            heat: Heat::Serious,
            memory: MemoryPressure::Normal,
        });
        assert!(guard.current().workers_held());
        assert!(changes.has_changed().unwrap());
        assert_eq!(parse_fake("critical\n").heat, Heat::Critical);
        assert_eq!(parse_fake("memory").memory, MemoryPressure::Warning);
        assert_eq!(
            parse_fake("memory-critical").memory,
            MemoryPressure::Critical
        );
        assert_eq!(parse_fake("calm"), MachineLoad::default());
    }
}
