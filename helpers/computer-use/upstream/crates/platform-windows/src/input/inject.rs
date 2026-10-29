//! Background input guards and checked coordinate helpers.
//!
//! Semantic UIA and native posted-message routes are preferred by callers.
//! The final pen/touch injection fallback is refused: Windows pointer input
//! and mouse promotion do not provide a stationary-system-cursor guarantee.
//! Never hide an excursion with cursor clipping, hiding, or a snap-back.

use anyhow::{bail, Result};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
    LockSetForegroundWindow, SetForegroundWindow, SetWindowLongPtrW, GA_ROOT, GWL_EXSTYLE,
    LSFW_LOCK, LSFW_UNLOCK, WS_EX_NOACTIVATE,
};

#[path = "foreground_watch.rs"]
pub(crate) mod foreground_watch;

use foreground_watch::policy::ForegroundLockState;

static FOREGROUND_LOCK_STATE: Mutex<ForegroundLockState> = Mutex::new(ForegroundLockState {
    holders: 0,
    locked: false,
    generation: 0,
});

/// Prevent other processes from taking the foreground during a background
/// launch. Windows automatically clears this lock on genuine user input; Drop
/// still balances the documented unlock call and overlapping driver launches.
pub struct ForegroundLockGuard {
    held: bool,
    generation: u64,
}

impl ForegroundLockGuard {
    pub fn acquire() -> Self {
        let mut state = FOREGROUND_LOCK_STATE.lock().unwrap();
        if state.holders == 0 {
            state.locked = unsafe { LockSetForegroundWindow(LSFW_LOCK) }.is_ok();
            if state.locked {
                tracing::debug!(target: "launch_app.focus_lock", "locked foreground changes during background launch");
            } else {
                tracing::warn!(target: "launch_app.focus_lock", "could not lock foreground changes during background launch");
            }
        }
        if state.locked {
            state.add_holder();
        }
        Self {
            held: state.locked,
            generation: state.generation,
        }
    }

    pub fn acquired(&self) -> bool {
        let state = FOREGROUND_LOCK_STATE.lock().unwrap();
        self.held && state.locked && self.generation == state.generation
    }
}

/// Revoke every overlapping launch lease on physical input. Stale drops
/// cannot unlock a later generation, and no existing lease can re-lock it.
static LAST_PHYSICAL_INPUT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn foreground_input_time() -> Option<u32> {
    LAST_PHYSICAL_INPUT
        .load(std::sync::atomic::Ordering::Acquire)
        .checked_sub(1)
        .map(|time| time as u32)
}

pub(crate) fn revoke_foreground_lock(time: u32) {
    LAST_PHYSICAL_INPUT.store(u64::from(time) + 1, std::sync::atomic::Ordering::Release);
    let mut state = FOREGROUND_LOCK_STATE.lock().unwrap();
    if state.revoke() {
        let _ = unsafe { LockSetForegroundWindow(LSFW_UNLOCK) };
    }
}

pub(crate) fn foreground_generation() -> u64 {
    FOREGROUND_LOCK_STATE.lock().unwrap().generation
}

/// Win32 has no atomic conditional foreground swap. Check both selection and
/// physical-input generation at each boundary, never resampling a new victim.
pub(crate) unsafe fn hand_back_foreground(
    prior: HWND,
    displaced: HWND,
    allowed: impl Fn() -> bool,
) -> bool {
    if !allowed() || unsafe { GetForegroundWindow() } != displaced {
        return false;
    }
    // If prevention already failed, relinquish every live lease before a
    // conditional hand-back. Keep holder counts, so none of these leases can
    // re-lock; physical input can still revoke their generation at any time.
    {
        let mut state = FOREGROUND_LOCK_STATE.lock().unwrap();
        if unsafe { GetForegroundWindow() } != displaced {
            return false;
        }
        if state.locked {
            let _ = unsafe { LockSetForegroundWindow(LSFW_UNLOCK) };
            state.locked = false;
        }
    }
    if !allowed() || unsafe { GetForegroundWindow() } != displaced {
        return false;
    }
    let my_tid = unsafe { GetCurrentThreadId() };
    let tid = unsafe { GetWindowThreadProcessId(displaced, None) };
    let attached = tid != 0 && tid != my_tid;
    if !allowed() || unsafe { GetForegroundWindow() } != displaced {
        return false;
    }
    if attached {
        let _ = unsafe { AttachThreadInput(my_tid, tid, true) };
    }
    if allowed() && unsafe { GetForegroundWindow() } == displaced {
        let _ = unsafe { SetForegroundWindow(prior) };
    }
    if attached {
        let _ = unsafe { AttachThreadInput(my_tid, tid, false) };
    }
    (unsafe { GetForegroundWindow() }) == prior
}

impl Drop for ForegroundLockGuard {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        let mut state = FOREGROUND_LOCK_STATE.lock().unwrap();
        if state.release(self.generation) {
            let _ = unsafe { LockSetForegroundWindow(LSFW_UNLOCK) };
            tracing::debug!(target: "launch_app.focus_lock", "unlocked foreground changes after background launch");
        }
    }
}

/// Bring `target` to the foreground using the AttachThreadInput trick, which
/// inherits the current foreground thread's FG-lock token so the swap is
/// honored even on a foreground-locked session without UIAccess (mirrors the
/// `bring_to_front` tool). Single attach, no retry loop — bounded. Returns
/// whether `target` actually became foreground.
pub(crate) unsafe fn force_foreground_attached(target: HWND) -> bool {
    let cur = GetForegroundWindow();
    if cur == target {
        return true;
    }
    let my_tid = GetCurrentThreadId();
    let mut pid = 0u32;
    let cur_tid = GetWindowThreadProcessId(cur, Some(&mut pid));
    let attached = cur_tid != 0 && cur_tid != my_tid;
    if attached {
        let _ = AttachThreadInput(my_tid, cur_tid, true);
    }
    let _ = SetForegroundWindow(target);
    if attached {
        let _ = AttachThreadInput(my_tid, cur_tid, false);
    }
    GetForegroundWindow() == target
}

/// Retry an explicit visible foreground transition after a reserved no-name
/// key grants this process the most-recent-input token. The key has no
/// application meaning; the retry count keeps the transition bounded.
pub(crate) unsafe fn force_foreground_assisted(target: HWND) -> (bool, bool) {
    if unsafe { force_foreground_attached(target) } {
        return (true, false);
    }

    const VK_NONAME: u8 = 0xFC;
    unsafe {
        keybd_event(VK_NONAME, 0, KEYBD_EVENT_FLAGS(0), 0);
        keybd_event(VK_NONAME, 0, KEYEVENTF_KEYUP, 0);
    }
    for _ in 0..3 {
        if unsafe { force_foreground_attached(target) } {
            return (true, true);
        }
        sleep(Duration::from_millis(25));
    }
    (false, true)
}

/// Suppress system activation of the target and its owner while background
/// input is in flight. Explicit app activation may bypass this style; the
/// event watch handles observed activation and late dialog dismissal.
/// Leases are shared so overlapping run_steps do not clear each other's bit.
pub struct NoActivateGuard {
    styles: Vec<(isize, u32, u32)>,
    _watch: foreground_watch::ForegroundWatch,
}

struct StyleLease {
    holders: usize,
    added: bool,
}

static STYLE_LEASES: Mutex<std::collections::BTreeMap<(isize, u32, u32), StyleLease>> =
    Mutex::new(std::collections::BTreeMap::new());

impl NoActivateGuard {
    pub fn arm(hwnd: HWND) -> Self {
        use windows::Win32::UI::WindowsAndMessaging::GA_ROOTOWNER;
        let prior = unsafe { GetForegroundWindow() }.0 as usize;
        let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
        let root = if root.0.is_null() { hwnd } else { root };
        let owner = unsafe { GetAncestor(root, GA_ROOTOWNER) };
        let mut styles = Vec::new();
        let mut pids = Vec::new();
        let mut leases = STYLE_LEASES.lock().unwrap();
        for window in [root, owner] {
            if window.0.is_null() || window.0 as usize == prior {
                continue;
            }
            let mut pid = 0;
            let tid = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
            if pid == 0 || tid == 0 {
                continue;
            }
            let key = (window.0 as isize, pid, tid);
            if styles.contains(&key) {
                continue;
            }
            pids.push(pid);
            let lease = leases.entry(key).or_insert_with(|| {
                let current = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
                let want = WS_EX_NOACTIVATE.0 as isize;
                let added = current & want == 0
                    && unsafe {
                        SetWindowLongPtrW(window, GWL_EXSTYLE, current | want);
                        GetWindowLongPtrW(window, GWL_EXSTYLE) & want != 0
                    };
                StyleLease { holders: 0, added }
            });
            lease.holders += 1;
            styles.push(key);
        }
        drop(leases);
        pids.sort_unstable();
        pids.dedup();
        let watch = foreground_watch::ForegroundWatch::start(
            prior,
            foreground_watch::Target::Processes(
                pids,
                styles.iter().map(|key| key.0 as usize).collect(),
            ),
            true,
        );
        Self {
            styles,
            _watch: watch,
        }
    }
}

impl Drop for NoActivateGuard {
    fn drop(&mut self) {
        let mut leases = STYLE_LEASES.lock().unwrap();
        for key in &self.styles {
            let Some(lease) = leases.get_mut(key) else {
                continue;
            };
            lease.holders = lease.holders.saturating_sub(1);
            if lease.holders != 0 {
                continue;
            }
            let added = lease.added;
            leases.remove(key);
            let window = HWND(key.0 as *mut _);
            let mut pid = 0;
            let tid = unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
            // Do not modify an unrelated window if Windows recycled the HWND.
            if added && pid == key.1 && tid == key.2 {
                unsafe {
                    let current = GetWindowLongPtrW(window, GWL_EXSTYLE);
                    SetWindowLongPtrW(
                        window,
                        GWL_EXSTYLE,
                        current & !(WS_EX_NOACTIVATE.0 as isize),
                    );
                }
            }
        }
    }
}

// Guard even unindexed key/text calls: a posted Return can dismiss a dialog
// asynchronously and let its owner activate after PostMessage has returned.
pub fn guarded_post_key(hwnd: u64, key: &str, modifiers: &[&str]) -> Result<()> {
    let _guard = NoActivateGuard::arm(HWND(hwnd as *mut _));
    super::keyboard::post_key(hwnd, key, modifiers)
}

pub fn guarded_post_key_to(
    hwnd: u64,
    child: Option<u64>,
    key: &str,
    modifiers: &[&str],
) -> Result<()> {
    let _guard = NoActivateGuard::arm(HWND(hwnd as *mut _));
    super::keyboard::post_key_to(hwnd, child, key, modifiers)
}

pub fn guarded_post_char(hwnd: u64, ch: char) -> Result<()> {
    let _guard = NoActivateGuard::arm(HWND(hwnd as *mut _));
    super::keyboard::post_char(hwnd, ch)
}

pub fn guarded_post_type_text(hwnd: u64, text: &str) -> Result<()> {
    let _guard = NoActivateGuard::arm(HWND(hwnd as *mut _));
    super::keyboard::post_type_text(hwnd, text)
}

pub fn guarded_post_type_text_with_delay(hwnd: u64, text: &str, inter_char_ms: u64) -> Result<()> {
    let _guard = NoActivateGuard::arm(HWND(hwnd as *mut _));
    super::keyboard::post_type_text_with_delay(hwnd, text, inter_char_ms)
}

/// This is only the final injection rung. Callers retain their earlier UIA
/// and native message routes; those do not inject a system pointing device.
pub fn inject_click_screen(
    _target: u64,
    _sx: i32,
    _sy: i32,
    _count: usize,
    _button: &str,
) -> Result<()> {
    bail!("background_unavailable: Windows cannot guarantee this synthetic pointer click without moving your pointer. No click was sent. Use an accessible element or ask the user to click.")
}

/// True when screen point `(x, y)` lies within `hwnd`'s window rectangle.
///
/// Guards **element_index** clicks: an element's cached center can fall outside
/// its own window when the element is scrolled out of a ScrollViewer or pushed
/// off-screen (e.g. a tall form on a small display). Tapping the raw coordinate
/// then lands on whatever is actually there — the taskbar, the desktop, another
/// window — instead of the intended element. The click tool turns a `false`
/// here into a clear error rather than clicking the wrong target. Returns `true`
/// (fail-open) when the rect can't be read, so legitimate clicks are never
/// blocked by a transient query failure.
pub fn point_in_window_bounds(hwnd: u64, x: i32, y: i32) -> bool {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
    if hwnd == 0 {
        return true;
    }
    let mut r = RECT::default();
    let ok = unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r).is_ok() };
    if !ok {
        return true;
    }
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn point_in_rect(x: i32, y: i32, left: i32, top: i32, width: i32, height: i32) -> bool {
    if width <= 0 || height <= 0 {
        return false;
    }
    let (x, y, left, top, width, height) = (
        i64::from(x),
        i64::from(y),
        i64::from(left),
        i64::from(top),
        i64::from(width),
        i64::from(height),
    );
    x >= left && x < left + width && y >= top && y < top + height
}

/// True when `(x, y)` belongs to the live Windows virtual desktop.
///
/// Unlike a fixed negative-coordinate threshold, this admits every layout the
/// OS actually reports. It also rejects the iconic `(-32000, -32000)` region
/// when a transient `IsIconic` query races with UIA/GetWindowRect state.
pub fn point_on_virtual_desktop(x: i32, y: i32) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    unsafe {
        point_in_rect(
            x,
            y,
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// True when `hwnd` is minimized (iconic).
///
/// Authoritative counterpart to [`is_iconic_sentinel_point`]: the capture path
/// already refuses iconic windows outright (see `capture.rs`), and the input
/// path needs the same signal so an element action can fail loudly instead of
/// posting into the off-screen iconic region.
pub fn window_is_iconic(hwnd: u64) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;
    if hwnd == 0 {
        return false;
    }
    unsafe { IsIconic(HWND(hwnd as *mut _)).as_bool() }
}

#[cfg(test)]
mod iconic_sentinel_tests {
    use super::*;

    /// The exact rect Win32 hands back for a minimized window, as documented on
    /// the capture-path guard in `capture.rs`.
    const ICONIC_RECT: (i32, i32, i32, i32) = (-32000, -32000, -31840, -31972);

    #[test]
    fn virtual_desktop_membership_has_no_magic_negative_ceiling() {
        assert!(point_in_rect(-31920, 50, -40000, 0, 50000, 2000));
    }

    #[test]
    fn iconic_center_is_outside_an_ordinary_virtual_desktop() {
        let (left, top, right, bottom) = ICONIC_RECT;
        let (cx, cy) = ((left + right) / 2, (top + bottom) / 2);
        assert!(!point_in_rect(cx, cy, -2560, -1080, 6400, 3240));
    }

    #[test]
    fn iconic_value_on_either_axis_is_outside_the_desktop() {
        assert!(!point_in_rect(640, -32000, -2560, -1080, 6400, 3240));
        assert!(!point_in_rect(-32000, 480, -2560, -1080, 6400, 3240));
    }

    #[test]
    fn real_negative_origin_monitor_points_remain_valid() {
        for (x, y) in [
            (-1920, 0),
            (-1795, 383),
            (-2560, 0),
            (0, -1080),
            (-1920, -1080),
            (0, 0),
            (1920, 1080),
        ] {
            assert!(point_in_rect(x, y, -2560, -1080, 6400, 3240));
        }
    }

    #[test]
    fn virtual_desktop_edges_and_invalid_extents_fail_closed() {
        assert!(point_in_rect(-1795, 383, -2560, -1080, 6400, 3240));
        assert!(!point_in_rect(-2561, 383, -2560, -1080, 6400, 3240));
        assert!(!point_in_rect(3840, 0, -2560, -1080, 6400, 3240));
        assert!(!point_in_rect(0, 0, 0, 0, 0, 1080));
        assert!(!point_in_rect(0, 0, 0, 0, 1920, -1));
    }
}

/// Native posted drags remain available to callers where supported. The
/// pointer-injection fallback cannot promise to leave the user's cursor alone.
pub fn inject_drag_screen(
    _target: u64,
    _sx0: i32,
    _sy0: i32,
    _sx1: i32,
    _sy1: i32,
    _steps: usize,
    _button: &str,
) -> Result<()> {
    bail!("background_unavailable: Windows cannot guarantee this synthetic pointer drag without moving your pointer. No drag was sent. Use set_value for an accessible slider or ask the user to drag.")
}

// (Removed inject_key_cloaked / inject_text_cloaked.) The macOS-aligned
// contract forbids a `background` actuation from grabbing focus — even a
// *cloaked* (hidden) one. Keyboard/text that the target's input stack would
// drop in the background (VCL/classic-Win32 accelerators, Chromium key-combos,
// WPF/terminal text) is now reported as `background_unavailable`; the agent
// escalates to `delivery_mode:"foreground"`, which uses the explicit
// SetForegroundWindow path (send_key_synthesized / send_text_synthesized).
