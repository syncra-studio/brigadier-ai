//! The process facts the supervisor needs on Windows and Linux: which
//! executable a pid runs, when it started, whether it is alive, and how to
//! stop it. macOS has its own in `supervisor` and `serve`.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// The executable process `pid` runs.
#[cfg(target_os = "linux")]
pub fn executable(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// When process `pid` started: its start in clock ticks after boot
/// (`/proc/<pid>/stat`, field 22) plus the boot time (`/proc/stat`, `btime`).
#[cfg(target_os = "linux")]
pub fn start_time(pid: u32) -> Option<SystemTime> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may hold spaces and parentheses; fields follow the last `)`.
    let after_name = &stat[stat.rfind(')')? + 1..];
    // Field 3 (state) is the first after the name, so field 22 is the 20th.
    let ticks: u64 = after_name.split_whitespace().nth(19)?.parse().ok()?;
    let boot: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    // SAFETY: sysconf has no preconditions.
    let per_second = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) })
        .ok()
        .filter(|ticks| *ticks > 0)?;
    Some(
        SystemTime::UNIX_EPOCH
            + Duration::from_secs(boot)
            + Duration::from_millis(ticks.saturating_mul(1000) / per_second),
    )
}

/// Whether process `pid` still exists.
#[cfg(target_os = "linux")]
pub fn is_alive(pid: u32) -> bool {
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(raw, 0) == 0 }
}

/// Ask process `pid` to stop (`SIGTERM`), or end it (`SIGKILL`).
#[cfg(target_os = "linux")]
pub fn terminate(pid: u32, force: bool) {
    let Ok(raw) = libc::pid_t::try_from(pid) else {
        return;
    };
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    // SAFETY: the caller checked that `pid` is our daemon.
    unsafe { libc::kill(raw, signal) };
}

/// The executable process `pid` runs.
#[cfg(windows)]
pub fn executable(pid: u32) -> Option<PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
    let process = Handle::open(pid, false)?;
    let mut buffer = vec![0u16; 32_768];
    let mut length = u32::try_from(buffer.len()).ok()?;
    // SAFETY: the buffer outlives the call and its length is passed in.
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .ok()?;
    buffer.truncate(usize::try_from(length).ok()?);
    Some(PathBuf::from(String::from_utf16(&buffer).ok()?))
}

/// When process `pid` started.
#[cfg(windows)]
pub fn start_time(pid: u32) -> Option<SystemTime> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetProcessTimes;
    let process = Handle::open(pid, false)?;
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: all four out pointers are valid.
    unsafe { GetProcessTimes(process.0, &mut created, &mut exited, &mut kernel, &mut user) }
        .ok()?;
    // 100 ns intervals since 1601-01-01.
    let intervals = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    const UNIX_EPOCH_IN_INTERVALS: u64 = 116_444_736_000_000_000;
    let since_epoch = intervals.checked_sub(UNIX_EPOCH_IN_INTERVALS)?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(since_epoch.saturating_mul(100)))
}

/// Whether process `pid` still runs.
#[cfg(windows)]
pub fn is_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::WAIT_TIMEOUT;
    use windows::Win32::System::Threading::WaitForSingleObject;
    let Some(process) = Handle::open(pid, true) else {
        return false;
    };
    // SAFETY: the handle is valid; a zero timeout only polls.
    unsafe { WaitForSingleObject(process.0, 0) == WAIT_TIMEOUT }
}

/// End process `pid`. Windows has no polite signal for a windowless
/// process, so both forms end it.
#[cfg(windows)]
pub fn terminate(pid: u32, _force: bool) {
    use windows::Win32::System::Threading::TerminateProcess;
    if let Some(process) = Handle::open_for_termination(pid) {
        // SAFETY: the handle carries PROCESS_TERMINATE; the caller checked
        // that `pid` is our daemon.
        let _ = unsafe { TerminateProcess(process.0, 0) };
    }
}

/// A process handle, closed when dropped.
#[cfg(windows)]
struct Handle(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Handle {
    fn open(pid: u32, synchronize: bool) -> Option<Self> {
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        };
        let mut access = PROCESS_QUERY_LIMITED_INFORMATION;
        if synchronize {
            access |= PROCESS_SYNCHRONIZE;
        }
        // SAFETY: the handle is checked and closed on drop.
        unsafe { OpenProcess(access, false, pid) }.ok().map(Self)
    }

    fn open_for_termination(pid: u32) -> Option<Self> {
        use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE};
        // SAFETY: the handle is checked and closed on drop.
        unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }
            .ok()
            .map(Self)
    }
}

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by this value and not closed yet.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_known() {
        let pid = std::process::id();
        assert_eq!(executable(pid), std::env::current_exe().ok());
        let started = start_time(pid).expect("start time");
        assert!(started <= SystemTime::now());
        assert!(is_alive(pid));
    }
}
