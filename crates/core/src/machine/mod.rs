//! Processes by their pid and start time, so a later process reusing a pid is never taken
//! for one Brigadier knows (a preview's tree).

use brigadier_sandbox::Platform;
use serde::{Deserialize, Serialize};

/// A process by its pid and start time, so a later process reusing the pid is never taken
/// for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Proc {
    pub pid: u32,
    /// Start time, in ms since the Unix epoch.
    pub started_ms: i64,
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
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

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
}
