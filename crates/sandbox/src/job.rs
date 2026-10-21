//! A Windows job object around a child, so what it started goes with it: Windows has no process
//! group to signal, and a process's children outlive it with no parent to find them by.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::process::Child;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject,
};

/// A job holding a child and every process it starts from then on (nested jobs, so a caller's
/// own job doesn't stop it). Dropping it leaves them running; [`Self::terminate`] ends them.
pub struct ProcessJob(HANDLE);

impl ProcessJob {
    /// Puts `child` in a new job. What it started before this call isn't in it, so call it
    /// right after the spawn.
    pub fn around(child: &Child) -> io::Result<Self> {
        // SAFETY: no attributes and no name: an unnamed job with the default security.
        #[allow(unsafe_code)]
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(handle);
        // SAFETY: both handles are open: the job's own and the child's, which `child` holds
        // until it is dropped.
        #[allow(unsafe_code)]
        let assigned = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Ends every process still in the job, the child too if it runs.
    pub fn terminate(&self) {
        // SAFETY: the job's handle is open until drop. A job whose processes are all gone
        // terminates nothing, which is no error.
        #[allow(unsafe_code)]
        unsafe {
            TerminateJobObject(self.0, 1)
        };
    }
}

impl Drop for ProcessJob {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by CreateJobObjectW and is closed exactly once.
        #[allow(unsafe_code)]
        unsafe {
            CloseHandle(self.0)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    /// A child's own child, left running after the child is gone, goes with the job.
    #[test]
    fn terminating_the_job_ends_what_the_child_left_running() {
        // cmd waits for a line, then starts a sort that outlives it: the sort starts once cmd is
        // in the job. It holds cmd's output pipe (an inherited handle) and waits for the end of
        // its input, which the test keeps open.
        let mut child = Command::new("cmd")
            .args([
                "/C",
                r"set /p go= & start /B %SystemRoot%\System32\sort.exe",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let job = ProcessJob::around(&child).unwrap();
        let mut input = child.stdin.take().unwrap();
        input.write_all(b"go\r\n").unwrap();
        let mut out = child.stdout.take().unwrap();
        child.wait().unwrap();
        let (send, got) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = io::copy(&mut out, &mut io::sink());
            let _ = send.send(());
        });
        // The sort still holds the pipe.
        assert!(
            got.recv_timeout(std::time::Duration::from_millis(500))
                .is_err()
        );
        job.terminate();
        got.recv_timeout(std::time::Duration::from_secs(5))
            .expect("the pipe closes once the job is terminated");
        drop(input);
    }
}
