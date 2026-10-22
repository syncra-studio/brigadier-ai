//! The machine guard: Brigadier holds back new workers while the machine struggles,
//! automatically and with no setting (PLAN.md §10.7).
//!
//! - Serious heat or critical memory pressure holds new workers. They wait with a grey row in
//!   the thread and start when it eases. Work already running is left alone.
//! - Commands are the CLIs' own: Brigadier never queues, stops or pauses them.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_sandbox::{Heat, MachineLoad, MemoryPressure, Platform};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::model::MachineStepReason;

/// How often the watch reads the machine's load.
pub(crate) const TICK: Duration = Duration::from_secs(2);
/// How long something waiting for the machine sleeps between looks at whether it is still
/// wanted.
pub(crate) const RECHECK: Duration = Duration::from_secs(20);

/// A process by its pid and start time, so a later process reusing the pid is never taken
/// for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Proc {
    pub pid: u32,
    /// Start time, in ms since the Unix epoch.
    pub started_ms: i64,
}

/// The guard, read every [`TICK`].
pub(crate) struct MachineWatch {
    pub(crate) guard: MachineGuard,
}

impl MachineWatch {
    pub(crate) fn new(platform: Arc<dyn Platform>) -> Self {
        Self {
            guard: MachineGuard::new(platform),
        }
    }

    /// Waits until workers can start or what holds them changes (heat, memory), at most `max`;
    /// whether the machine eased.
    pub(crate) async fn eased_within(&self, max: Duration) -> bool {
        let mut changes = self.guard.subscribe();
        let reason = MachineStepReason::from_load(self.guard.current());
        let _ = tokio::time::timeout(
            max,
            changes.wait_for(|load| {
                !load.workers_held() || MachineStepReason::from_load(*load) != reason
            }),
        )
        .await;
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

/// Whether `proc` still is the process it was (alive and not a zombie, the same start time).
pub(crate) fn still(platform: &dyn Platform, proc: Proc) -> bool {
    platform.processes().is_alive(proc.pid)
        && !platform.processes().is_zombie(proc.pid)
        && proc_of(platform, proc.pid)
            .is_some_and(|now| (now.started_ms - proc.started_ms).abs() < 1_000)
}

/// Whether `proc` has certainly ended: it no longer runs, it is a zombie, or its PID now
/// belongs to a process started at another time. Unlike `!still`, a check that fails (its
/// start time unreadable) proves nothing, so a caller keeps what it knows about `proc`.
pub(crate) fn gone(platform: &dyn Platform, proc: Proc) -> bool {
    let processes = platform.processes();
    !processes.is_alive(proc.pid)
        || processes.is_zombie(proc.pid)
        || processes
            .start_time_ms(proc.pid)
            .is_ok_and(|now| (now.round() as i64 - proc.started_ms).abs() >= 1_000)
}

/// Lets go on `members` (what was suspended under `root`), and anything still below `root`,
/// children before their parents.
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

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::Ordering;
    use std::time::Instant;

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

    /// How long a process the test started may take to get where it waits for. Only a hang
    /// takes this long: a process starting, stopping or going on is quick on a calm machine,
    /// but each step is many times slower on one loaded far past its cores. Shorter on CI
    /// ([`crate::test_patience`]).
    static PATIENCE: std::sync::LazyLock<Duration> = std::sync::LazyLock::new(crate::test_patience);

    fn wait_for<T>(mut found: impl FnMut() -> Option<T>) -> T {
        let deadline = Instant::now() + *PATIENCE;
        loop {
            if let Some(value) = found() {
                return value;
            }
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[tokio::test]
    async fn memory_warning_allows_workers_but_critical_holds_them() {
        let dir = TempDir::new();
        let watch = MachineWatch::new(platform(dir.path()));
        watch.guard.fake(parse_fake("memory"));
        assert!(watch.eased_within(Duration::ZERO).await);
        watch.guard.fake(parse_fake("memory-critical"));
        assert!(!watch.eased_within(Duration::ZERO).await);
        watch.guard.fake(parse_fake("hot"));
        assert!(!watch.eased_within(Duration::ZERO).await);
        watch.guard.fake(parse_fake("calm"));
        assert!(watch.eased_within(Duration::ZERO).await);
    }

    /// The native platform, with a chosen PID failing identity reads after a countdown.
    pub(crate) struct Unreadable(
        pub Arc<dyn Platform>,
        pub u32,
        pub std::sync::atomic::AtomicUsize,
    );

    impl Platform for Unreadable {
        fn name(&self) -> &'static str {
            self.0.name()
        }
        fn paths(&self) -> &brigadier_sandbox::AppPaths {
            self.0.paths()
        }
        fn private_fs(&self) -> &dyn brigadier_sandbox::PrivateFs {
            self.0.private_fs()
        }
        fn processes(&self) -> &dyn brigadier_sandbox::Processes {
            self
        }
        fn credentials(&self) -> &dyn brigadier_sandbox::CredentialStore {
            self.0.credentials()
        }
        fn shell(&self) -> &dyn brigadier_sandbox::Shell {
            self.0.shell()
        }
        fn sandbox(&self) -> &dyn brigadier_sandbox::Sandbox {
            self.0.sandbox()
        }
        fn machine(&self) -> &dyn brigadier_sandbox::Machine {
            self.0.machine()
        }
    }

    impl brigadier_sandbox::Processes for Unreadable {
        fn spawn_detached(
            &self,
            spec: &brigadier_sandbox::SpawnSpec,
        ) -> brigadier_sandbox::Result<brigadier_sandbox::DetachedChild> {
            self.0.processes().spawn_detached(spec)
        }
        fn piped_command(&self, spec: &brigadier_sandbox::SpawnSpec) -> Command {
            self.0.processes().piped_command(spec)
        }
        fn is_alive(&self, pid: u32) -> bool {
            self.0.processes().is_alive(pid)
        }
        fn is_zombie(&self, pid: u32) -> bool {
            self.0.processes().is_zombie(pid)
        }
        fn is_stopped(&self, pid: u32) -> bool {
            self.0.processes().is_stopped(pid)
        }
        fn terminate(&self, pid: u32) -> brigadier_sandbox::Result<()> {
            self.0.processes().terminate(pid)
        }
        fn kill_tree(&self, pid: u32) -> brigadier_sandbox::Result<()> {
            self.0.processes().kill_tree(pid)
        }
        fn kill_group(&self, pid: u32) -> brigadier_sandbox::Result<()> {
            self.0.processes().kill_group(pid)
        }
        fn descendants(&self, pid: u32) -> brigadier_sandbox::Result<Vec<u32>> {
            self.0.processes().descendants(pid)
        }
        fn children(&self, pid: u32) -> brigadier_sandbox::Result<Vec<u32>> {
            self.0.processes().children(pid)
        }
        fn group_of(&self, pid: u32) -> Option<u32> {
            self.0.processes().group_of(pid)
        }
        fn command_line(&self, pid: u32) -> Option<Vec<String>> {
            self.0.processes().command_line(pid)
        }
        fn suspend(&self, pid: u32) -> brigadier_sandbox::Result<()> {
            self.0.processes().suspend(pid)
        }
        fn resume(&self, pid: u32) -> brigadier_sandbox::Result<()> {
            self.0.processes().resume(pid)
        }
        fn cpu_time_ms(&self, pid: u32) -> Option<u64> {
            self.0.processes().cpu_time_ms(pid)
        }
        fn start_time_ms(&self, pid: u32) -> brigadier_sandbox::Result<f64> {
            if pid != self.1
                || self
                    .2
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                        left.checked_sub(1)
                    })
                    .is_ok()
            {
                return self.0.processes().start_time_ms(pid);
            }
            Err(std::io::Error::other("unreadable").into())
        }
        fn in_dir(&self, dir: &Path) -> brigadier_sandbox::Result<Vec<u32>> {
            self.0.processes().in_dir(dir)
        }
    }

    #[test]
    fn only_an_ended_zombie_or_reused_process_is_gone() {
        let dir = TempDir::new();
        let platform = platform(dir.path());
        let mut sleeper = Command::new("/bin/sleep").arg("300").spawn().unwrap();
        let proc = proc_of(&*platform, sleeper.id()).unwrap();
        assert!(still(&*platform, proc) && !gone(&*platform, proc));
        let reused = Proc {
            started_ms: proc.started_ms - 5_000,
            ..proc
        };
        assert!(!still(&*platform, reused) && gone(&*platform, reused));
        // A failed identity check is no proof the process ended.
        let unreadable = Unreadable(platform.clone(), proc.pid, 0.into());
        assert!(!still(&unreadable, proc) && !gone(&unreadable, proc));

        // Exited but not reaped yet: a signal still reaches it, and only its state shows it
        // has ended.
        sleeper.kill().unwrap();
        wait_for(|| platform.processes().is_zombie(proc.pid).then_some(()));
        assert!(platform.processes().is_alive(proc.pid));
        assert!(!still(&*platform, proc));
        assert!(gone(&*platform, proc) && gone(&unreadable, proc));
        sleeper.wait().unwrap();
        assert!(gone(&*platform, proc));
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
