//! Linux: everything the daemon needs to run. The worker sandbox, credential storage and
//! login-shell resolution arrive with the Linux platform phase.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;

use nix::unistd::{SysconfVar, sysconf};

use crate::machine::{heat_from_thermal_zones, memory_from_psi};
use crate::unix::{self, UnixPrivateFs};
use crate::{
    AppPaths, CredentialStore, DetachedChild, Machine, MachineLoad, Platform, PrivateFs, Processes,
    Result, Sandbox, SandboxPolicy, Shell, SpawnSpec, unsupported,
};

const NAME: &str = "linux";

pub(crate) struct Linux {
    paths: AppPaths,
}

impl Linux {
    pub(crate) fn new(paths: AppPaths) -> Self {
        Self { paths }
    }
}

impl Platform for Linux {
    fn name(&self) -> &'static str {
        NAME
    }
    fn paths(&self) -> &AppPaths {
        &self.paths
    }
    fn private_fs(&self) -> &dyn PrivateFs {
        &UnixPrivateFs
    }
    fn processes(&self) -> &dyn Processes {
        &LinuxProcesses
    }
    fn credentials(&self) -> &dyn CredentialStore {
        &Unsupported
    }
    fn shell(&self) -> &dyn Shell {
        &Unsupported
    }
    fn sandbox(&self) -> &dyn Sandbox {
        &Unsupported
    }
    fn machine(&self) -> &dyn Machine {
        &LinuxMachine
    }
}

/// Thermal zones against their trip points, and memory pressure stall information.
struct LinuxMachine;

impl Machine for LinuxMachine {
    fn load(&self) -> MachineLoad {
        MachineLoad {
            heat: heat_from_thermal_zones(std::path::Path::new("/sys/class/thermal")),
            memory: std::fs::read_to_string("/proc/pressure/memory")
                .map(|text| memory_from_psi(&text))
                .unwrap_or_default(),
        }
    }
}

struct LinuxProcesses;

impl Processes for LinuxProcesses {
    fn spawn_detached(&self, spec: &SpawnSpec) -> Result<DetachedChild> {
        unix::spawn_detached(spec)
    }
    fn piped_command(&self, spec: &SpawnSpec) -> std::process::Command {
        unix::piped_command(spec)
    }
    fn is_alive(&self, pid: u32) -> bool {
        unix::is_alive(pid)
    }
    fn is_zombie(&self, pid: u32) -> bool {
        // Field 3 (state), the first after the closing parenthesis of comm.
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rfind(')')
                .and_then(|index| stat[index + 1..].split_whitespace().next())
                == Some("Z")
        })
    }
    fn terminate(&self, pid: u32) -> Result<()> {
        unix::terminate(pid)
    }
    fn kill_tree(&self, pid: u32) -> Result<()> {
        unix::kill_tree(pid, child_pids)
    }
    fn kill_group(&self, pid: u32) -> Result<()> {
        unix::kill_group(pid)
    }
    fn descendants(&self, pid: u32) -> Result<Vec<u32>> {
        Ok(unix::descendants(pid, child_pids))
    }
    fn children(&self, pid: u32) -> Result<Vec<u32>> {
        Ok(child_pids(pid))
    }
    fn group_of(&self, pid: u32) -> Option<u32> {
        unix::group_of(pid)
    }
    fn command_line(&self, pid: u32) -> Option<Vec<String>> {
        let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let args: Vec<String> = raw
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8_lossy(arg).into_owned())
            .collect();
        (!args.is_empty()).then_some(args)
    }
    fn suspend(&self, pid: u32) -> Result<()> {
        unix::suspend(pid)
    }
    fn resume(&self, pid: u32) -> Result<()> {
        unix::resume(pid)
    }
    fn cpu_time_ms(&self, pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Fields 14 and 15 (utime, stime), counted from the closing parenthesis of comm.
        let mut fields = stat[stat.rfind(')')? + 1..].split_whitespace().skip(11);
        let user: u64 = fields.next()?.parse().ok()?;
        let system: u64 = fields.next()?.parse().ok()?;
        let ticks_per_sec = sysconf(SysconfVar::CLK_TCK)
            .ok()
            .flatten()
            .filter(|ticks| *ticks > 0)? as u64;
        Some((user + system) * 1000 / ticks_per_sec)
    }
    fn in_dir(&self, dir: &std::path::Path) -> Result<Vec<u32>> {
        let dir = dir.canonicalize()?;
        let own = std::process::id();
        let mut pids = Vec::new();
        for entry in std::fs::read_dir("/proc")?.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == own || pid <= 1 {
                continue;
            }
            // Other users' processes are unreadable, and processes may exit meanwhile.
            if std::fs::read_link(entry.path().join("cwd")).is_ok_and(|cwd| cwd.starts_with(&dir)) {
                pids.push(pid);
            }
        }
        Ok(pids)
    }
    fn start_time_ms(&self, pid: u32) -> Result<f64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
        // Field 2 (comm) may contain spaces, so count fields from the closing parenthesis.
        let after_comm = stat
            .rfind(')')
            .map(|index| &stat[index + 1..])
            .ok_or_else(bad_proc)?;
        let start_ticks: f64 = after_comm
            .split_whitespace()
            .nth(19)
            .and_then(|field| field.parse().ok())
            .ok_or_else(bad_proc)?;
        let boot_secs: f64 = std::fs::read_to_string("/proc/stat")?
            .lines()
            .find_map(|line| line.strip_prefix("btime "))
            .and_then(|value| value.trim().parse().ok())
            .ok_or_else(bad_proc)?;
        let ticks_per_sec = sysconf(SysconfVar::CLK_TCK)
            .ok()
            .flatten()
            .filter(|ticks| *ticks > 0)
            .ok_or_else(bad_proc)? as f64;
        Ok(boot_secs * 1000.0 + start_ticks * 1000.0 / ticks_per_sec)
    }
}

fn bad_proc() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "unexpected /proc format")
}

struct Unsupported;

impl CredentialStore for Unsupported {
    fn set(&self, _account: &str, _secret: &[u8]) -> Result<()> {
        unsupported("credential storage", NAME)
    }
    fn get(&self, _account: &str) -> Result<Option<Vec<u8>>> {
        unsupported("credential storage", NAME)
    }
    fn delete(&self, _account: &str) -> Result<()> {
        unsupported("credential storage", NAME)
    }
}

impl Shell for Unsupported {
    fn login_shell(&self) -> Result<PathBuf> {
        unsupported("login shell resolution", NAME)
    }
    fn login_environment(&self) -> Result<BTreeMap<OsString, OsString>> {
        unsupported("login shell resolution", NAME)
    }
}

impl Sandbox for Unsupported {
    fn confine(&self, _spec: SpawnSpec, _policy: &SandboxPolicy) -> Result<SpawnSpec> {
        unsupported("the worker sandbox", NAME)
    }
}

/// A process's children, from each of its threads' `children` list in procfs.
fn child_pids(pid: u32) -> Vec<u32> {
    let Ok(threads) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    threads
        .flatten()
        .filter_map(|thread| std::fs::read_to_string(thread.path().join("children")).ok())
        .flat_map(|children| {
            children
                .split_whitespace()
                .filter_map(|child| child.parse().ok())
                .collect::<Vec<u32>>()
        })
        .collect()
}
