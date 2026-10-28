//! Cross-platform child reaping. Kills every spawned child on drop; on Windows
//! also assigns them to a kill-on-close Job Object so the OS reaps the whole
//! tree even on panic / SIGKILL / Ctrl-C.

use std::process::{Child, Command};
use std::time::Duration;

/// Owns spawned children and externally-discovered pids (e.g. a packaged app's
/// broker-launched window process), killing them all on drop. Prompt cleanup
/// between tests; the Windows Job Object is the hard-kill backstop.
pub struct ChildReaper {
    children: Vec<Child>,
    pids: Vec<u32>,
}

impl ChildReaper {
    pub fn new() -> Self {
        ChildReaper {
            children: Vec::new(),
            pids: Vec::new(),
        }
    }

    /// Spawn `cmd` into the kill-on-close job (Windows) and own the child.
    pub fn spawn(&mut self, cmd: &mut Command) -> std::io::Result<()> {
        let child = spawn_in_job(cmd)?;
        self.children.push(child);
        Ok(())
    }

    /// Take ownership of an already-spawned child (assigning it to the job on
    /// Windows). Use when you spawned via [`spawn_in_job`] to grab its pipes
    /// first, then hand the remainder here.
    pub fn push(&mut self, child: Child) {
        self.children.push(child);
    }

    /// Track an external pid (and its whole tree) for teardown — packaged /
    /// broker-launched window processes that aren't our direct child.
    pub fn track_pid(&mut self, pid: u32) {
        self.pids.push(pid);
    }
}

impl Default for ChildReaper {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ChildReaper {
    fn drop(&mut self) {
        for &pid in &self.pids {
            tree_kill(pid);
        }
        for c in &mut self.children {
            tree_kill(c.id());
            let _ = c.kill();
            let _ = c.wait();
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Spawn a command, assigning it to the kill-on-close job on Windows so it can
/// never outlive the test process. On other platforms a plain spawn (the
/// [`ChildReaper`] still kills it on drop).
pub fn spawn_in_job(cmd: &mut Command) -> std::io::Result<Child> {
    #[cfg(not(target_os = "windows"))]
    {
        cmd.spawn()
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::{io::AsRawHandle, process::CommandExt};
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::System::JobObjects::*;
        static JOB: std::sync::OnceLock<Result<isize, String>> = std::sync::OnceLock::new();
        let job = JOB
            .get_or_init(|| unsafe {
                let job = CreateJobObjectW(None, None).map_err(|e| e.to_string())?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if let Err(error) = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of_val(&info) as u32,
                ) {
                    let _ = CloseHandle(job);
                    return Err(error.to_string());
                }
                Ok(job.0 as isize)
            })
            .as_ref()
            .map_err(|error| std::io::Error::other(error.clone()))?;
        // Console helpers must never open a visible terminal during a test.
        cmd.creation_flags(0x08000000);
        let mut child = cmd.spawn()?;
        if let Err(error) = unsafe {
            AssignProcessToJobObject(HANDLE(*job as *mut _), HANDLE(child.as_raw_handle()))
        } {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other(error.to_string()));
        }
        Ok(child)
    }
}

#[cfg(not(target_os = "windows"))]
fn tree_kill(pid: u32) {
    use std::process::Stdio;
    let _ = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(target_os = "windows")]
fn tree_kill(pid: u32) {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    let _ = Command::new("taskkill.exe")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(0x08000000)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}
