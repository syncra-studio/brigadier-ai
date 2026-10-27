//! Watch launch-time activation, including shell delegation to existing processes.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IsWindow,
};

#[derive(Default)]
struct State {
    restored: bool,
    failed: bool,
    ended: Option<Instant>,
}

pub(super) struct LaunchForegroundGuard(Arc<Mutex<State>>);

fn input_tick() -> Option<u32> {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetLastInputInfo(&mut info) }
        .as_bool()
        .then_some(info.dwTime)
}

fn window_process(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = vec![0u16; 32768];
        let mut len = path.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(path.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        Some(super::launch_policy::executable_name(
            &String::from_utf16_lossy(&path[..len as usize]),
        ))
    }
}

pub(super) fn associated_executable(url: &str) -> Option<String> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::UI::Shell::{AssocQueryStringW, ASSOCF_NONE, ASSOCSTR_EXECUTABLE};
    let protocol = url.split_once(':')?.0;
    let wide: Vec<u16> = protocol.encode_utf16().chain(Some(0)).collect();
    let mut output = vec![0u16; 32768];
    let mut len = output.len() as u32;
    unsafe {
        AssocQueryStringW(
            ASSOCF_NONE,
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
        let state = Arc::new(Mutex::new(State::default()));
        let worker = state.clone();
        // Capture before spawning so user input during the launch always disarms restoration.
        let initial_input = input_tick();
        let prior_pid = window_process(HWND(prior as *mut _));
        // Name resolution can take time. Do not undo a switch made before the watcher starts.
        if unsafe { GetForegroundWindow() } != HWND(prior as *mut _) {
            return Self(state);
        }
        std::thread::spawn(move || {
            let hard_deadline = Instant::now() + Duration::from_secs(20);
            loop {
                let ended = worker.lock().unwrap().ended;
                if Instant::now() >= hard_deadline
                    || ended.is_some_and(|end| end.elapsed() >= Duration::from_secs(3))
                    || initial_input.is_none()
                    || input_tick() != initial_input
                {
                    break;
                }
                let prior_hwnd = HWND(prior as *mut _);
                let foreground = unsafe { GetForegroundWindow() };
                let prior_valid = prior != 0
                    && unsafe { IsWindow(prior_hwnd) }.as_bool()
                    && window_process(prior_hwnd) == prior_pid;
                if !prior_valid {
                    break;
                }
                if foreground != prior_hwnd {
                    let matches = process_name(window_process(foreground))
                        .is_some_and(|name| expected.contains(&name));
                    if super::launch_policy::may_restore_foreground(
                        prior_valid,
                        input_tick() == initial_input,
                        false,
                        matches,
                    ) {
                        // No synthetic keypress: attach only for this verified hand-back.
                        let restored =
                            unsafe { crate::input::force_foreground_attached(prior_hwnd) };
                        let mut state = worker.lock().unwrap();
                        state.restored |= restored;
                        state.failed = !restored;
                    } else if !foreground.0.is_null() {
                        // Another app took over; preserve that switch for the rest of this launch.
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        Self(state)
    }

    pub(super) fn report(&self) -> (&'static str, bool) {
        let state = self.0.lock().unwrap();
        if state.failed {
            (
                "Windows activated the app; handing the foreground back could not be confirmed.",
                true,
            )
        } else if state.restored {
            (
                "Windows activated the app; the previous foreground window was restored.",
                false,
            )
        } else {
            ("No launch activation was observed; foreground hand-back remains guarded for late windows.", false)
        }
    }
}

impl Drop for LaunchForegroundGuard {
    fn drop(&mut self) {
        self.0.lock().unwrap().ended = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_never_claims_unconfirmed_handoff_succeeded() {
        let guard = LaunchForegroundGuard(Arc::new(Mutex::new(State {
            failed: true,
            ..State::default()
        })));
        assert!(guard.report().1);
        assert!(guard.report().0.contains("could not be confirmed"));
    }
}
