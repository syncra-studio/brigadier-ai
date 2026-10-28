//! Universal background mouse actuator: coordinate-routed pointer/touch
//! injection that delivers a click to whatever window sits under a screen
//! point **without** `SetForegroundWindow` and **without** moving the user's
//! mouse cursor.
//!
//! Why this exists: the PostMessage path (`mouse::post_click`) is invisible and
//! never raises, but Chromium/Electron/GTK/WPF content silently ignore posted
//! synthetic `WM_*BUTTON` messages — their input arrives through the *system
//! input queue*, not the per-window message queue. The historical fallback for
//! those was `send_click_synthesized` (SendInput + a `SetForegroundWindow`
//! swap), which is exactly the visible "flash" we want to eliminate.
//!
//! Touch injection routes by coordinate through the system input queue (so
//! Chromium et al. accept it; the OS promotes it to `WM_*BUTTON` for legacy
//! Win32 windows that don't consume `WM_POINTER`), and — per the RE in
//! `docs/windows-background-input-re-plan.md` §4.4 — the kernel injection path
//! (`NtUserInjectMouseInput`/`NtUserInjectTouchInput`) gates only on a
//! per-process injection-enable, NOT on the target being foreground. The one
//! residual is that a tap on an *inactive* top-level window can still trigger
//! click-activation; we contain that with `NoActivateGuard` (WS_EX_NOACTIVATE)
//! plus a `force_foreground_attached(prev_fg)` re-assertion of the USER's
//! foreground afterward. Per the macOS-aligned contract a background actuation
//! never raises/restacks the target: when coordinate-routed input can't reach
//! it (occluded at the point), the caller returns `background_unavailable`
//! rather than raising it (see `target_visible_at_point`).
//!
//! Scope: left-button taps (single/double/triple). Right/middle have no clean
//! touch mapping; callers fall back to their existing routing for those.

use anyhow::{bail, Result};
use core::ffi::c_void;
use std::sync::Mutex;
use std::thread::sleep;
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, HWND, POINT, RECT};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE,
    POINTER_FEEDBACK_DEFAULT, POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
};
use windows::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_FLAG_UP, POINTER_FLAG_UPDATE, POINTER_INFO, POINTER_PEN_INFO, POINTER_TOUCH_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetCursorPos, GetForegroundWindow, GetWindowLongPtrW, GetWindowThreadProcessId,
    IsWindow, LockSetForegroundWindow, SetCursorPos, SetForegroundWindow, SetWindowLongPtrW,
    WindowFromPoint, GA_ROOT, GWL_EXSTYLE, LSFW_LOCK, LSFW_UNLOCK, PT_PEN, PT_TOUCH,
    WS_EX_NOACTIVATE,
};

#[path = "foreground_watch.rs"]
pub(crate) mod foreground_watch;

#[derive(Default)]
struct ForegroundLockState {
    holders: usize,
    locked: bool,
}

static FOREGROUND_LOCK_STATE: Mutex<ForegroundLockState> = Mutex::new(ForegroundLockState {
    holders: 0,
    locked: false,
});

/// Prevent other processes from taking the foreground during a background
/// launch. Windows automatically clears this lock on genuine user input; Drop
/// still balances the documented unlock call and overlapping driver launches.
pub struct ForegroundLockGuard {
    held: bool,
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
            state.holders += 1;
        }
        Self { held: state.locked }
    }

    pub fn acquired(&self) -> bool {
        self.held
    }
}

impl Drop for ForegroundLockGuard {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        let mut state = FOREGROUND_LOCK_STATE.lock().unwrap();
        state.holders = state.holders.saturating_sub(1);
        if state.holders == 0 {
            let _ = unsafe { LockSetForegroundWindow(LSFW_UNLOCK) };
            state.locked = false;
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
            foreground_watch::Target::Processes(pids),
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

/// PEN_FLAG_BARREL (winuser.h) — pen barrel button held == secondary (right)
/// button. `penFlags` is a raw u32 in the bindings, so use the literal.
const PEN_FLAG_BARREL: u32 = 0x00000001;

// (Removed ZorderGuard.) The macOS-aligned contract forbids a `background`
// actuation from raising/restacking the target: coordinate-routed pen/touch
// only lands on the topmost VISIBLE window, so an occluded target is reported
// as `background_unavailable` (see `target_visible_at_point`) and the agent
// escalates to `delivery_mode:"foreground"` instead of the driver raising it.

/// One down→up **pen** tap at screen `(sx, sy)`. When `barrel` is set the pen's
/// barrel button is held for the contact, which the system maps to a secondary
/// (right) click — both for `WM_POINTER`-aware apps (Chromium/WPF/UWP) and via
/// pen→mouse promotion for legacy Win32. A fresh synthetic pen device is
/// created and destroyed per tap (right/middle clicks are rare).
fn pen_taps(sx: i32, sy: i32, barrel: bool, count: usize) -> Result<()> {
    unsafe {
        let dev = CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT)
            .map_err(|e| anyhow::anyhow!("CreateSyntheticPointerDevice(PEN): {e}"))?;
        let pen_flags = if barrel { PEN_FLAG_BARREL } else { 0 };
        let mk = |flags| POINTER_TYPE_INFO {
            r#type: PT_PEN,
            Anonymous: POINTER_TYPE_INFO_0 {
                penInfo: POINTER_PEN_INFO {
                    pointerInfo: POINTER_INFO {
                        pointerType: PT_PEN,
                        pointerId: 0,
                        pointerFlags: flags,
                        sourceDevice: HANDLE::default(),
                        hwndTarget: HWND::default(),
                        ptPixelLocation: POINT { x: sx, y: sy },
                        ..Default::default()
                    },
                    penFlags: pen_flags,
                    penMask: 0,
                    pressure: 512,
                    rotation: 0,
                    tiltX: 0,
                    tiltY: 0,
                },
            },
        };
        let down = mk(POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT);
        let up = mk(POINTER_FLAG_UP);
        // Reuse the SAME synthetic device for every tap. A double/triple click
        // is two/three down-up cycles from one digitizer; creating a fresh
        // device per tap (the old loop) fails the next
        // CreateSyntheticPointerDevice in quick succession — which is exactly
        // why background double_click on Chromium errored. See #1984.
        let mut result: Result<()> = Ok(());
        let n = count.max(1);
        for i in 0..n {
            let r1 = InjectSyntheticPointerInput(dev, &[down]);
            sleep(Duration::from_millis(25));
            let r2 = InjectSyntheticPointerInput(dev, &[up]);
            if let Err(e) = r1.and(r2) {
                result = Err(anyhow::anyhow!("InjectSyntheticPointerInput(pen): {e}"));
                break;
            }
            if i + 1 < n {
                sleep(Duration::from_millis(70));
            }
        }
        let _ = DestroySyntheticPointerDevice(dev);
        result?;
    }
    Ok(())
}

/// Inject a click at **screen** coordinates `(sx, sy)`, routed by the system to
/// whatever window is under that point — without a foreground swap and without
/// moving the user's cursor. The target is cloaked for the duration so any
/// click-activation raise stays invisible, then the user's foreground is
/// restored.
///
/// - `left`  → synthetic-pen primary tap (proven path; promoted to a left click
///   for non-pointer-aware apps, and accepted directly by Chromium/WPF/UWP).
/// - `right` → synthetic-pen tap with the barrel button held (secondary click).
/// - `middle`→ unsupported (no clean pointer mapping); returns `Err` so the
///   caller can fall back to its existing routing / structured error.
///
/// We use the same `CreateSyntheticPointerDevice`/`InjectSyntheticPointerInput`
/// path for both buttons — `InjectTouchInput` proved unreliable for left-clicks
/// on Chromium content (returned errors), whereas synthetic-pen injection lands
/// reliably and routes by coordinate with no foreground dependency.
pub fn inject_click_screen(
    target: u64,
    sx: i32,
    sy: i32,
    count: usize,
    button: &str,
) -> Result<()> {
    if target == 0 {
        bail!("inject_click_screen: null target window");
    }
    let target_h = HWND(target as *mut _);
    unsafe {
        if !IsWindow(target_h).as_bool() {
            bail!("inject_click_screen: invalid or stale target HWND");
        }
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target) {
        // Higher-integrity target: injection into its queue is blocked too.
        bail!(msg);
    }

    let barrel = match button {
        "left" => false,
        "right" => true,
        other => bail!("background injection supports left/right buttons only (got {other:?})"),
    };

    // macOS-aligned contract: a `background` actuation NEVER fronts or raises
    // (mirrors macOS CGEvent-to-pid, which never touches z-order/focus). Synthetic
    // pen/touch is coordinate-routed to the TOP-MOST VISIBLE window at the point,
    // so if the target is occluded there we'd silently click the occluder. We do
    // NOT raise to win the hit-test (that's the foreground rung's job, which the
    // agent opts into). Instead bail — the caller surfaces `background_unavailable`
    // and the agent escalates to `delivery_mode:"foreground"`.
    if unsafe { !target_visible_at_point(target_h, sx, sy) } {
        bail!(
            "background coordinate injection cannot reach this target at ({sx},{sy}) \
             — it is occluded by another window (raising it would break the \
             no-foreground contract). Escalate to delivery_mode:\"foreground\"."
        );
    }
    // Remember who the user had in front so we can re-assert it below.
    let prev_fg = unsafe { GetForegroundWindow() };
    // Make the target non-activatable for the click so click-activation can't
    // steal foreground. No z-order raise.
    let result = {
        let _noact = NoActivateGuard::arm(target_h);
        // One synthetic device does all `count` taps (single/double/triple click).
        pen_taps(sx, sy, barrel, count)
    };
    // `WS_EX_NOACTIVATE` is NOT categorical against a Chromium/Electron content
    // window that calls `SetForegroundWindow(self)` from its (async) click
    // handler. Re-assert the USER's foreground (this restores focus where the
    // user left it — it never raises the target). Short settle + repeat to win
    // the race against the async self-activation.
    unsafe {
        if result.is_ok() && !prev_fg.0.is_null() && prev_fg != target_h {
            force_foreground_attached(prev_fg);
            sleep(Duration::from_millis(12));
            force_foreground_attached(prev_fg);
        }
    }
    result
}

/// True when `target`'s top-level root is the window actually visible at screen
/// point `(sx, sy)` — i.e. coordinate-routed pen/touch there would land on it
/// rather than an occluder. Used to enforce the macOS-aligned rule that a
/// `background` actuation never raises: when this is false the injection bails
/// and the tool returns `background_unavailable` (escalate to foreground).
unsafe fn target_visible_at_point(target: HWND, sx: i32, sy: i32) -> bool {
    let top = WindowFromPoint(POINT { x: sx, y: sy });
    if top.0.is_null() {
        return false;
    }
    let top_root = GetAncestor(top, GA_ROOT);
    let target_root = GetAncestor(target, GA_ROOT);
    !top_root.0.is_null() && top_root == target_root
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

/// One pen press-drag-release from screen `(sx0,sy0)` to `(sx1,sy1)`, with
/// `steps` interpolated in-contact UPDATE points between the down and the up.
/// A single synthetic pen device is created for the whole stroke. The barrel
/// button is held when `barrel` is set (secondary-button drag).
fn pen_drag(sx0: i32, sy0: i32, sx1: i32, sy1: i32, steps: usize, barrel: bool) -> Result<()> {
    unsafe {
        let dev = CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT)
            .map_err(|e| anyhow::anyhow!("CreateSyntheticPointerDevice(PEN): {e}"))?;
        let pen_flags = if barrel { PEN_FLAG_BARREL } else { 0 };
        let mk = |flags, x: i32, y: i32| POINTER_TYPE_INFO {
            r#type: PT_PEN,
            Anonymous: POINTER_TYPE_INFO_0 {
                penInfo: POINTER_PEN_INFO {
                    pointerInfo: POINTER_INFO {
                        pointerType: PT_PEN,
                        pointerId: 0,
                        pointerFlags: flags,
                        sourceDevice: HANDLE::default(),
                        hwndTarget: HWND::default(),
                        ptPixelLocation: POINT { x, y },
                        ..Default::default()
                    },
                    penFlags: pen_flags,
                    penMask: 0,
                    pressure: 512,
                    rotation: 0,
                    tiltX: 0,
                    tiltY: 0,
                },
            },
        };
        // Press at the start.
        let down = mk(
            POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
            sx0,
            sy0,
        );
        let mut res = InjectSyntheticPointerInput(dev, &[down]);
        // Interpolated in-contact moves so frameworks that gate drag-tracking on
        // motion (rather than a single down→up) see a continuous stroke.
        let steps = steps.max(1);
        for i in 1..=steps {
            sleep(Duration::from_millis(8));
            let t = i as f64 / steps as f64;
            let x = sx0 + ((sx1 - sx0) as f64 * t).round() as i32;
            let y = sy0 + ((sy1 - sy0) as f64 * t).round() as i32;
            let mv = mk(
                POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                x,
                y,
            );
            res = res.and(InjectSyntheticPointerInput(dev, &[mv]));
        }
        // Release at the end.
        sleep(Duration::from_millis(8));
        let up = mk(POINTER_FLAG_UP, sx1, sy1);
        res = res.and(InjectSyntheticPointerInput(dev, &[up]));
        let _ = DestroySyntheticPointerDevice(dev);
        res.map_err(|e| anyhow::anyhow!("InjectSyntheticPointerInput(pen drag): {e}"))?;
    }
    Ok(())
}

/// A **persistent** synthetic touch digitizer, created once and never
/// destroyed. This is load-bearing: a transient (per-stroke) device is gone
/// before WPF's WISP stylus stack can bind to it, so the OS falls back to
/// legacy touch→mouse promotion — which drags the user's cursor to the
/// contact. A *standing* digitizer is enumerated as a real tablet, so WPF (and
/// other stylus/pointer-aware frameworks) consume the contact as touch/stylus
/// and promote it to mouse INTERNALLY, without the OS moving the system cursor.
static TOUCH_DEV: Mutex<isize> = Mutex::new(0);

/// One **touch** press-drag-release from screen `(sx0,sy0)` to `(sx1,sy1)` with
/// `steps` interpolated in-contact moves, via the persistent [`TOUCH_DEV`].
/// Unlike a pen (an absolute *cursor* device — injecting one drags the user's
/// mouse pointer along), a touch contact from a standing digitizer is consumed
/// as touch/stylus and does NOT move the user's cursor. Serialized on the
/// single shared device (one stroke at a time across all sessions).
fn touch_drag(sx0: i32, sy0: i32, sx1: i32, sy1: i32, steps: usize) -> Result<()> {
    let mut dev_guard = TOUCH_DEV.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        // A non-pointer-aware window (WPF) makes the OS promote the PRIMARY touch
        // contact to a mouse event, which drags the system cursor to the contact
        // — and the OS gates delivery on the cursor actually reaching it, so the
        // move can't be prevented from a background process (pinning/clipping the
        // cursor just drops the input). What we CAN do is snap the cursor back to
        // exactly where the user left it the instant the stroke ends, so the net
        // displacement is zero and (with a fast, few-step stroke) the excursion
        // is a brief flick rather than a sustained drag. Pointer-aware targets
        // (Chromium) never promote, so the cursor never moves and this restore is
        // a harmless no-op.
        let mut cpos = POINT::default();
        let have_cpos = GetCursorPos(&mut cpos).is_ok();
        let dev = if *dev_guard != 0 {
            HSYNTHETICPOINTERDEVICE(*dev_guard as *mut c_void)
        } else {
            let d = CreateSyntheticPointerDevice(PT_TOUCH, 1, POINTER_FEEDBACK_DEFAULT)
                .map_err(|e| anyhow::anyhow!("CreateSyntheticPointerDevice(TOUCH): {e}"))?;
            *dev_guard = d.0 as isize;
            d
        };
        let mk = |flags, x: i32, y: i32| POINTER_TYPE_INFO {
            r#type: PT_TOUCH,
            Anonymous: POINTER_TYPE_INFO_0 {
                touchInfo: POINTER_TOUCH_INFO {
                    pointerInfo: POINTER_INFO {
                        pointerType: PT_TOUCH,
                        pointerId: 0,
                        pointerFlags: flags,
                        sourceDevice: HANDLE::default(),
                        hwndTarget: HWND::default(),
                        ptPixelLocation: POINT { x, y },
                        ..Default::default()
                    },
                    touchFlags: 0,
                    touchMask: 0,
                    rcContact: RECT {
                        left: x - 2,
                        top: y - 2,
                        right: x + 2,
                        bottom: y + 2,
                    },
                    rcContactRaw: RECT {
                        left: x - 2,
                        top: y - 2,
                        right: x + 2,
                        bottom: y + 2,
                    },
                    orientation: 0,
                    pressure: 512,
                },
            },
        };
        // Fast stroke: line-tool canvases only need down→up (the segment is the
        // straight line between them), so a few in-contact frames with a tiny
        // dwell is plenty — and the shorter the stroke, the briefer the cursor
        // excursion before we snap it back.
        let down = mk(
            POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
            sx0,
            sy0,
        );
        let mut res = InjectSyntheticPointerInput(dev, &[down]);
        let steps = steps.clamp(1, 3);
        for i in 1..=steps {
            sleep(Duration::from_millis(2));
            let t = i as f64 / steps as f64;
            let x = sx0 + ((sx1 - sx0) as f64 * t).round() as i32;
            let y = sy0 + ((sy1 - sy0) as f64 * t).round() as i32;
            let mv = mk(
                POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
                x,
                y,
            );
            res = res.and(InjectSyntheticPointerInput(dev, &[mv]));
        }
        sleep(Duration::from_millis(2));
        let up = mk(POINTER_FLAG_UP, sx1, sy1);
        res = res.and(InjectSyntheticPointerInput(dev, &[up]));
        // Snap the cursor back to where the user left it. The OS processes the
        // promoted mouse messages slightly after injection, so a single restore
        // right after the `up` can be overrun by that late move — settle briefly,
        // then restore, and restore once more to win the race. No-op for
        // pointer-aware targets (Chromium) that never moved the cursor.
        if have_cpos {
            let _ = SetCursorPos(cpos.x, cpos.y);
            sleep(Duration::from_millis(12));
            let _ = SetCursorPos(cpos.x, cpos.y);
        }
        // device intentionally NOT destroyed — see TOUCH_DEV.
        res.map_err(|e| anyhow::anyhow!("InjectSyntheticPointerInput(touch drag): {e}"))?;
    }
    Ok(())
}

/// Inject a press-drag-release at **screen** coordinates, routed by the system
/// to whatever window is under the path — without a foreground swap and without
/// moving the user's cursor. This is the background fallback for canvases whose
/// content (Chromium/WPF/GTK) silently drops a PostMessage drag: synthetic-pen
/// input arrives through the system input queue and is accepted directly
/// (Chromium/WPF) or promoted to mouse for legacy Win32. `left` → primary
/// stroke, `right` → barrel-held secondary stroke; the target is held
/// non-activatable + cloaked so any transient raise stays invisible.
pub fn inject_drag_screen(
    target: u64,
    sx0: i32,
    sy0: i32,
    sx1: i32,
    sy1: i32,
    steps: usize,
    button: &str,
) -> Result<()> {
    if target == 0 {
        bail!("inject_drag_screen: null target window");
    }
    let target_h = HWND(target as *mut _);
    unsafe {
        if !IsWindow(target_h).as_bool() {
            bail!("inject_drag_screen: invalid or stale target HWND");
        }
    }
    if let Some(msg) = crate::input::post_message_blocked_by_uipi(target) {
        bail!(msg);
    }
    let barrel = match button {
        "left" => false,
        "right" => true,
        other => bail!("background injection supports left/right buttons only (got {other:?})"),
    };
    // macOS-aligned contract: a `background` drag NEVER fronts/raises. WPF's
    // Wisp stylus stack only PROCESSES injected input while the window is the
    // ACTIVE foreground (verified by RE) — and background is not allowed to
    // force that — so a WPF drag is simply undeliverable in background. Bail so
    // the tool returns background_unavailable (escalate to foreground).
    if crate::input::delivery::is_wpf_target_window(target) {
        bail!(
            "background drag is not deliverable to a WPF target — its Wisp stylus \
             stack only processes injected input while the window is foreground, \
             which background must not force. Escalate to delivery_mode:\"foreground\"."
        );
    }
    // Coordinate-routed touch/pen lands on the topmost VISIBLE window at the
    // start point. If the target is occluded there, bail rather than raise it.
    if unsafe { !target_visible_at_point(target_h, sx0, sy0) } {
        bail!(
            "background drag cannot reach this target at the start point ({sx0},{sy0}) \
             — it is occluded by another window. Escalate to delivery_mode:\"foreground\"."
        );
    }
    let prev_fg = unsafe { GetForegroundWindow() };
    // Left drag → touch contact (coordinate-routed). Right/barrel drag has no
    // touch equivalent, so fall back to a pen (rare).
    let stroke = |()| {
        if barrel {
            pen_drag(sx0, sy0, sx1, sy1, steps, true)
        } else {
            touch_drag(sx0, sy0, sx1, sy1, steps)
        }
    };
    // Chromium/GTK: pointer-aware, process injection in the background — hold
    // non-activatable (no raise), inject, then re-assert the user's foreground.
    let r = {
        let _noact = NoActivateGuard::arm(target_h);
        stroke(())
    };
    unsafe {
        if !prev_fg.0.is_null() && prev_fg != target_h {
            force_foreground_attached(prev_fg);
        }
    }
    r
}

// (Removed inject_key_cloaked / inject_text_cloaked.) The macOS-aligned
// contract forbids a `background` actuation from grabbing focus — even a
// *cloaked* (hidden) one. Keyboard/text that the target's input stack would
// drop in the background (VCL/classic-Win32 accelerators, Chromium key-combos,
// WPF/terminal text) is now reported as `background_unavailable`; the agent
// escalates to `delivery_mode:"foreground"`, which uses the explicit
// SetForegroundWindow path (send_key_synthesized / send_text_synthesized).
