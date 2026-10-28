//! Launch-time foreground observation uses the same event-backed guard as input.
use crate::input::inject::foreground_watch::{ForegroundWatch, Target};

pub(super) struct LaunchForegroundGuard {
    watch: ForegroundWatch,
    _lock: crate::input::ForegroundLockGuard,
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
        let lock = crate::input::ForegroundLockGuard::acquire();
        Self {
            watch: ForegroundWatch::start(prior, Target::launch(expected)),
            _lock: lock,
        }
    }

    #[allow(dead_code)] // Used by the launch call-site integration.
    pub(super) fn register_pid(&self, pid: u32) {
        self.watch.register_pid(pid);
    }

    pub(super) fn report(&self) -> (&'static str, bool) {
        self.watch.report()
    }
}
