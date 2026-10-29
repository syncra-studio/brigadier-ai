//! Foreground-steal bypass for UWP / XAML / WinUI UIA activation calls.
//!
//! UWP/XAML/WinUI apps self-foreground during UIA `InvokePattern.Invoke`
//! (and `ExpandCollapse.Expand`, `Toggle.Toggle`, `SelectionItem.Select` —
//! anything the XAML message-loop processes as an input-like event). The
//! XAML host unconditionally calls `SetForegroundWindow(self)` when handling
//! those, stealing focus from whatever the user had on top.
//!
//! Wrapping the call in `EnableWindow(host, FALSE) / call / EnableWindow(host, TRUE)`
//! silently suppresses the self-activation while letting the UIA pattern call
//! still execute — UIA pattern delivery uses the kernel accessibility channel,
//! not the input queue gated by `EnableWindow`.
//!
//! Empirical evidence (`flash-repro/14-multi-uwp-v3.ps1`, 2026-05-24):
//!   - Baseline UIA Invoke against UWP Calculator num5 button: user's
//!     foreground window dropped below the target in 91% of poller samples.
//!   - With this bypass: 0/507 z-drops across Calculator, Clock, Settings.
//!
//! Chromium / Electron hosts (`Chrome_WidgetWin_*`) exhibit the identical
//! self-foreground during UIA `Invoke` and are covered by the same shield (the
//! gate also accepts `is_chromium_target_window`). On Windows their content
//! window calls `SetForegroundWindow(self)` from the Invoke handler — measured
//! 7/8 background ax-bg actions stole focus before this; the `EnableWindow`
//! shield blocks it because a disabled top-level cannot become foreground while
//! the Invoke still lands over the a11y channel.
//!
//! Non-UWP / non-Chromium classic Win32 apps don't exhibit the bug
//! (`flash-repro/15-non-uwp.ps1`, Notepad: 0/45 baseline z-drops). The bypass
//! is therefore gated on `is_xaml_host_hwnd || is_chromium_target_window` and is
//! a no-op for other hosts.

use std::collections::BTreeMap;
use std::sync::Mutex;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetWindowThreadProcessId, GA_ROOT};

/// RAII guard that disables a window on construction and restores its
/// original enabled-state when the last overlapping guard drops. Leases are
/// shared across UIA worker threads, including calls that finish asynchronously.
pub struct DisabledHwndGuard {
    key: Option<(isize, u32, u32)>,
}

struct DisableLease {
    holders: usize,
    was_enabled: bool,
}

static DISABLE_LEASES: Mutex<BTreeMap<(isize, u32, u32), DisableLease>> =
    Mutex::new(BTreeMap::new());

fn window_key(hwnd: HWND) -> Option<(isize, u32, u32)> {
    let mut pid = 0;
    let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    (pid != 0 && tid != 0).then_some((hwnd.0 as isize, pid, tid))
}

impl DisabledHwndGuard {
    /// Disable `hwnd` for the lifetime of the guard. No-op for null HWND.
    pub fn disable(hwnd: HWND) -> Self {
        let Some(key) = window_key(hwnd) else {
            return Self { key: None };
        };
        let mut leases = DISABLE_LEASES.lock().unwrap_or_else(|e| e.into_inner());
        let lease = leases.entry(key).or_insert_with(|| {
            // EnableWindow returns nonzero iff the window was previously disabled.
            let was_enabled = !unsafe { EnableWindow(hwnd, false).as_bool() };
            DisableLease {
                holders: 0,
                was_enabled,
            }
        });
        lease.holders += 1;
        Self { key: Some(key) }
    }
}

impl Drop for DisabledHwndGuard {
    fn drop(&mut self) {
        let Some(key) = self.key else { return };
        let mut leases = DISABLE_LEASES.lock().unwrap_or_else(|e| e.into_inner());
        let Some(lease) = leases.get_mut(&key) else {
            return;
        };
        lease.holders -= 1;
        if lease.holders != 0 {
            return;
        }
        let was_enabled = lease.was_enabled;
        leases.remove(&key);
        let hwnd = HWND(key.0 as *mut _);
        // A destroyed/recycled window must not inherit another window's restoration.
        if was_enabled && window_key(hwnd) == Some(key) {
            unsafe {
                let _ = EnableWindow(hwnd, true);
            }
        }
    }
}

/// Wrap an activation closure (Invoke / Expand / Toggle / SelectionItem.Select)
/// in a UWP foreground-steal bypass.
///
/// `host_hwnd` is the top-level HWND of the window that contains the target
/// UIA element. When it identifies as an XAML host (`is_xaml_host_hwnd`), the
/// HWND is disabled for the duration of `action`. For non-XAML / classic
/// Win32 hosts the closure runs unmodified — empirically those don't
/// self-foreground via the input-queue path that EnableWindow gates.
///
/// **Known limitation, WPF Buttons / TextBoxes:** WPF's automation peers
/// call `UIElement.Focus()` synchronously during the UIA Invoke /
/// ValuePattern.SetValue handler. `Focus()` routes through
/// `SetForegroundWindow`, which is NOT gated by EnableWindow. The bypass
/// has no effect there, and the daemon WILL transiently steal foreground
/// from the user. Restoring foreground from a non-UIAccess process is
/// blocked by the foreground-lock, so the only mitigation is to spawn
/// cua-driver-uia.exe (UIAccess-manifested worker) and route UIA
/// activations through it. See PR #1699 bg-modality tests for the
/// regression guards.
pub fn run_with_uwp_bypass<T>(host_hwnd: isize, action: impl FnOnce() -> T) -> T {
    let _guard = make_guard(host_hwnd);
    action()
}

fn make_guard(host_hwnd: isize) -> Option<DisabledHwndGuard> {
    if host_hwnd == 0 {
        return None;
    }
    // XAML/UWP/WinUI hosts self-foreground during UIA pattern handling (the
    // original case). Chromium/Electron hosts (`Chrome_WidgetWin_*`) exhibit the
    // SAME bug: their UIA `InvokePattern.Invoke` handler reaches the browser's
    // focus path and calls `SetForegroundWindow(self)`, stealing focus from the
    // user's window on a *background* click (measured 7/8 ax-bg actions stole on
    // Windows — Chromium-specific; macOS WKWebView / Linux Electron hold).
    // `WS_EX_NOACTIVATE` (the injection path's `NoActivateGuard`) does NOT stop
    // an explicit self-`SetForegroundWindow`, but the `EnableWindow` shield does:
    // a *disabled* top-level cannot be made the foreground window, while the UIA
    // Invoke still lands (it's delivered over the kernel accessibility channel,
    // not the input queue `EnableWindow` gates). Same mechanism, same 0-z-drop
    // result as UWP — so gate the shield on Chromium too.
    let shielded = crate::input::is_xaml_host_hwnd(host_hwnd as u64)
        || crate::input::is_chromium_target_window(host_hwnd as u64);
    if !shielded {
        return None;
    }
    let h = HWND(host_hwnd as *mut _);
    // Defensive: walk up to the root in case the caller handed us a child
    // HWND inside the XAML host's HWND tree. UWP elements always disable
    // cleanly via the AppFrame root.
    let root = unsafe { GetAncestor(h, GA_ROOT) };
    let target = if root.0.is_null() { h } else { root };
    // Disabling the user's foreground window can itself deactivate it. An
    // already-front target needs no shield against self-activation.
    if unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() } == target {
        return None;
    }
    Some(DisabledHwndGuard::disable(target))
}

#[cfg(test)]
mod disable_lease_tests {
    use super::*;
    use windows::core::w;
    use windows::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, PeekMessageW, MSG, PM_REMOVE,
        WINDOW_EX_STYLE, WS_POPUP,
    };

    struct HiddenWindow(HWND);
    impl HiddenWindow {
        fn new() -> Self {
            Self(unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    w!("STATIC"),
                    w!(""),
                    WS_POPUP,
                    0,
                    0,
                    10,
                    10,
                    None,
                    None,
                    None,
                    None,
                )
                .unwrap()
            })
        }
        fn enabled(&self) -> bool {
            unsafe { IsWindowEnabled(self.0).as_bool() }
        }
    }
    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }

    #[test]
    fn disable_lease_restores_only_after_last_drop_in_either_order() {
        for first_created_first_dropped in [true, false] {
            let window = HiddenWindow::new();
            let first = DisabledHwndGuard::disable(window.0);
            let second = DisabledHwndGuard::disable(window.0);
            assert!(!window.enabled());
            let (first_drop, last_drop) = if first_created_first_dropped {
                (first, second)
            } else {
                (second, first)
            };
            drop(first_drop);
            assert!(
                !window.enabled(),
                "another holder still shields this window"
            );
            drop(last_drop);
            assert!(
                window.enabled(),
                "the initial state must survive overlapping calls"
            );
        }
        let window = HiddenWindow::new();
        unsafe {
            let _ = EnableWindow(window.0, false);
        }
        let first = DisabledHwndGuard::disable(window.0);
        let second = DisabledHwndGuard::disable(window.0);
        drop(first);
        drop(second);
        assert!(
            !window.enabled(),
            "do not enable an originally modal/disabled window"
        );
    }

    #[test]
    fn disable_lease_is_shared_between_worker_threads() {
        let window = HiddenWindow::new();
        let first = DisabledHwndGuard::disable(window.0);
        let hwnd = window.0 .0 as isize;
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let second = DisabledHwndGuard::disable(HWND(hwnd as *mut _));
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(second);
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        drop(first);
        assert!(!window.enabled(), "the other thread still owns its lease");
        release_tx.send(()).unwrap();
        // EnableWindow sends WM_ENABLE to this owning thread; pump it while
        // the final holder restores the state from its worker thread.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !worker.is_finished() && std::time::Instant::now() < deadline {
            let mut message = MSG::default();
            unsafe {
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            worker.is_finished(),
            "cross-thread EnableWindow did not finish"
        );
        worker.join().unwrap();
        assert!(window.enabled());
    }
}
