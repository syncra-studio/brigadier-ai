//! Launch-time foreground observation uses the same event-backed guard as input.
use crate::input::inject::foreground_watch::{ForegroundWatch, Target};
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct LaunchForegroundGuard {
    watch: ForegroundWatch,
    reported_foreground: AtomicUsize,
}

pub(super) fn associated_executable(url: &str) -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_IS_PROTOCOL, ASSOCSTR_EXECUTABLE};
    // ASSOCF_IS_PROTOCOL resolves the current user's protocol choice; a plain
    // class lookup can select a different browser from Windows Settings.
    // https://learn.microsoft.com/en-us/windows/win32/shell/assocf-str
    let protocol = url.split_once(':')?.0;
    let wide: Vec<u16> = protocol.encode_utf16().chain(Some(0)).collect();
    let mut output = vec![0u16; 32768];
    let mut len = output.len() as u32;
    unsafe {
        AssocQueryStringW(
            ASSOCF_IS_PROTOCOL,
            ASSOCSTR_EXECUTABLE,
            PCWSTR(wide.as_ptr()),
            PCWSTR::null(),
            PWSTR(output.as_mut_ptr()),
            &mut len,
        )
        .ok()
        .ok()?;
    }
    let executable = String::from_utf16_lossy(&output[..len.saturating_sub(1) as usize]);
    (!executable.is_empty()).then_some(executable)
}

impl LaunchForegroundGuard {
    pub(super) fn observe(prior: usize, expected: Vec<String>) -> Self {
        Self {
            watch: ForegroundWatch::start_locked_observing(prior, Target::launch(expected)),
            reported_foreground: AtomicUsize::new(0),
        }
    }
    pub(super) fn start(prior: usize, expected: Vec<String>) -> Self {
        // Shell/Store registrations delegate to differently named window hosts.
        // These names identify foreground attribution only, never an app pid.
        let prior_valid = prior != 0
            && unsafe {
                windows::Win32::UI::WindowsAndMessaging::IsWindow(windows::Win32::Foundation::HWND(
                    prior as *mut _,
                ))
                .as_bool()
            };
        let prior = if super::launch_policy::may_restore_foreground(prior_valid, true, false, true)
        {
            prior
        } else {
            0
        };
        // The watch holds the foreground lock until it ends, so a window that
        // appears after the reply remains guarded (locking may be refused).
        Self {
            watch: ForegroundWatch::start(prior, Target::launch(expected), true),
            reported_foreground: AtomicUsize::new(0),
        }
    }

    pub(super) fn register_pid(&self, pid: u32) {
        self.watch.register_pid(pid);
    }

    pub(super) fn report(&self) -> (&'static str, bool) {
        let (note, failed, foreground) = self.watch.report();
        self.reported_foreground
            .store(foreground, Ordering::Release);
        (note, failed)
    }

    pub(super) fn reported_foreground(&self) -> usize {
        self.reported_foreground.load(Ordering::Acquire)
    }
}
/// Task Manager relaunches itself elevated, so the system copy is refused
/// before any process starts. Live (2026-10-29): a direct non-elevated start
/// under the activation lock, and one with a per-process RunAsInvoker
/// compatibility layer inside a kill-on-close job, both ended in an elevated
/// window that the helper could not read or move and that covered the user's
/// window (58 s in the RunAsInvoker run, until the job was closed).
pub(super) fn task_manager_direct_path(target: &str) -> Result<Option<String>, String> {
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    let name = super::launch_policy::executable_name(target);
    if name != "taskmgr" && name != "task manager" {
        return Ok(None);
    }
    let mut buffer = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("Windows did not identify its trusted system directory. No Task Manager launch was sent.".into());
    }
    let executable = format!(
        "{}\\taskmgr.exe",
        String::from_utf16_lossy(&buffer[..length])
    );
    if target.contains(['\\', '/']) && !target.replace('/', "\\").eq_ignore_ascii_case(&executable)
    {
        return Ok(None);
    }
    Err("Task Manager raises itself to administrator after it starts, even when started without administrator rights, so this helper could not read or move its window and that window would cover your work. No process was started; open Task Manager yourself if you need it.".into())
}
