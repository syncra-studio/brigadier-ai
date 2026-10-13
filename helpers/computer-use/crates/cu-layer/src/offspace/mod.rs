//! Windows an app keeps on another Space.
//!
//! AppKit answers `AXWindows` with the windows on the active Space only. When
//! the user works in a full-screen app, an app launched in the background
//! opens its windows on the desktop Space, so they are missing from the list
//! even though the app still answers every request about them. Each window
//! element also has a remote token (pid, `'coco'`, a small element id), and an
//! element built from that token works like any other: its tree can be read
//! and its controls used, with nothing brought to the front.
//!
//! An element only gets its id once the app hands it to a client, and an app
//! hands out a window on another Space only while it is the app's main or
//! focused window. [`remember`] notes the id whenever such a window is seen
//! (at launch, on every lookup), so a document that another one opened after
//! it has replaced as main is still found.
//!
//! The app's window notifications are watched from the first touch on
//! ([`watch`]), so a document that was main only for a moment is remembered
//! too.
//!
//! [`install`] hands the driver [`resolve`] for the windows its `AXWindows`
//! read missed: the app's focused or main window when it is the one, else the
//! remembered id, else a scan of the app's element ids. Element ids are small
//! and stable for a window's life, so a found id is remembered, and a window
//! that a whole scan could not match (a hidden helper window, say) is not
//! scanned for again. A scan cut short by its time budget (a busy app) goes
//! on where it stopped next time instead.

mod watch;

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::data::{CFData, CFDataRef};
use platform_macos::ax::bindings::{
    _AXUIElementCreateWithRemoteToken, ax_get_window_id, copy_element_attr, copy_string_attr,
    AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetMessagingTimeout,
};

pub use watch::watch;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    /// The 20-byte remote token of an element (created; the caller releases
    /// it). An element only gets its id once it is handed to a client.
    fn _AXUIElementRemoteTokenCreate(element: AXUIElementRef) -> CFDataRef;
}

/// Element ids scanned for a missing window. Window elements sit far below
/// this in native and Electron apps alike.
const SCAN_LIMIT: u64 = 4096;
/// A scan stops here even if the window is still missing.
const SCAN_BUDGET: Duration = Duration::from_millis(400);
/// How long one probe may wait for the app; a missing id answers at once.
const PROBE_TIMEOUT_SECS: f32 = 0.1;
/// Apps remembered before the memory starts over.
const MAX_APPS: usize = 64;

/// Hands the driver [`resolve`] for windows on another Space. Once per
/// process; returns whether this call set it.
pub fn install() -> bool {
    platform_macos::ax::bindings::set_off_space_window_resolver(resolve)
}

/// True in a process that must not touch accessibility: this crate's unit
/// tests and the helper's own test processes.
fn ax_off() -> bool {
    cfg!(test) || crate::no_tcc::off()
}

#[derive(Default)]
struct AppMemory {
    /// Window id → element id of its remote token.
    found: HashMap<u32, u64>,
    /// Windows a full scan could not match.
    unmatched: HashSet<u32>,
    /// Where the last scan ran out of time, to go on from; 0 when it didn't.
    resume: u64,
}

impl AppMemory {
    /// After a scan from `started` that ran out of time at `stopped` (or went
    /// through, `None`) with `still_missing` unmatched: only a whole scan, from
    /// the first id to the last, rules a window out.
    fn after_scan(&mut self, started: u64, stopped: Option<u64>, still_missing: Vec<u32>) {
        match stopped {
            Some(at) => self.resume = at,
            None => {
                self.resume = 0;
                if started == 0 {
                    self.unmatched.extend(still_missing);
                }
            }
        }
    }
}

fn memory() -> &'static Mutex<HashMap<i32, AppMemory>> {
    static MEMORY: OnceLock<Mutex<HashMap<i32, AppMemory>>> = OnceLock::new();
    MEMORY.get_or_init(Default::default)
}

/// The memory of `pid`, starting over when too many apps are remembered.
fn app_memory(memory: &mut HashMap<i32, AppMemory>, pid: i32) -> &mut AppMemory {
    if memory.len() >= MAX_APPS && !memory.contains_key(&pid) {
        memory.clear();
    }
    memory.entry(pid).or_default()
}

/// Notes the token id of `window`, a window element of `pid`.
///
/// # Safety
/// `window` must be a valid AX element.
unsafe fn remember(pid: i32, window: AXUIElementRef) {
    let (Some(window_id), Some(element)) = (ax_get_window_id(window), element_id(window)) else {
        return;
    };
    let Ok(mut memory) = memory().lock() else {
        return;
    };
    let app = app_memory(&mut memory, pid);
    app.found.insert(window_id, element);
    app.unmatched.remove(&window_id);
}

/// Notes the app's focused and main windows, and starts watching its windows:
/// a window just opened is one of them only until the next one opens.
pub fn remember_front_windows(pid: i32) {
    watch(pid);
    if pid <= 0 || ax_off() {
        return;
    }
    for window in unsafe { front_windows(pid) } {
        unsafe { CFRelease(window as CFTypeRef) };
    }
}

/// The app's focused and main windows, each remembered. Retained.
///
/// # Safety
/// Calls the AX API; each returned element must be released by the caller.
unsafe fn front_windows(pid: i32) -> Vec<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return Vec::new();
    }
    // A busy app costs this call little: the note is only a head start.
    AXUIElementSetMessagingTimeout(app, 0.25);
    let mut windows = Vec::new();
    for attr in ["AXFocusedWindow", "AXMainWindow"] {
        if let Some(window) = copy_element_attr(app, attr) {
            remember(pid, window);
            windows.push(window);
        }
    }
    CFRelease(app as CFTypeRef);
    windows
}

/// The app's real windows (not zero-sized) from the window server, on any
/// Space.
fn server_windows(pid: i32) -> Vec<u32> {
    platform_macos::windows::all_windows()
        .into_iter()
        .filter(|w| w.pid == pid && w.layer == 0 && w.bounds.width > 1.0 && w.bounds.height > 1.0)
        .map(|w| w.window_id)
        .collect()
}

/// The window element for `window_id` of `pid` when `AXWindows` omits it,
/// retained for the caller to release: the driver's off-Space resolver.
fn resolve(pid: i32, window_id: u32) -> Option<AXUIElementRef> {
    if pid <= 0 {
        return None;
    }
    watch(pid);
    if !server_windows(pid).contains(&window_id) {
        return None;
    }
    unsafe {
        let mut front = None;
        for window in front_windows(pid) {
            if front.is_none() && ax_get_window_id(window) == Some(window_id) {
                front = Some(window);
            } else {
                CFRelease(window as CFTypeRef);
            }
        }
        if front.is_some() {
            return front;
        }
        let Ok(mut memory) = memory().lock() else {
            return None;
        };
        let app = app_memory(&mut memory, pid);
        if let Some(&element_id) = app.found.get(&window_id) {
            if let Some((element, _)) = window_element(pid, element_id, Some(window_id)) {
                return Some(element);
            }
            app.found.remove(&window_id);
        }
        if app.unmatched.contains(&window_id) {
            return None;
        }
        let deadline = Instant::now() + SCAN_BUDGET;
        let started = app.resume;
        let mut stopped = None;
        let mut hit = None;
        for element_id in started..SCAN_LIMIT {
            if Instant::now() >= deadline {
                stopped = Some(element_id);
                break;
            }
            let Some((element, window)) = window_element(pid, element_id, None) else {
                continue;
            };
            // Every window met on the way is noted, not only the one asked for.
            app.found.insert(window, element_id);
            app.unmatched.remove(&window);
            if window == window_id {
                hit = Some(element);
                break;
            }
            CFRelease(element as CFTypeRef);
        }
        let still_missing = if hit.is_some() {
            Vec::new()
        } else {
            vec![window_id]
        };
        app.after_scan(started, stopped, still_missing);
        hit
    }
}

/// The window element behind `element_id`, with its window id, if it is a
/// window (and the expected one, when given). Retained.
unsafe fn window_element(
    pid: i32,
    element_id: u64,
    expected: Option<u32>,
) -> Option<(AXUIElementRef, u32)> {
    let element = element_from_token(pid, element_id)?;
    AXUIElementSetMessagingTimeout(element, PROBE_TIMEOUT_SECS);
    let window = ax_get_window_id(element)
        .filter(|&id| expected.is_none_or(|e| e == id))
        .filter(|_| copy_string_attr(element, "AXRole").as_deref() == Some("AXWindow"));
    match window {
        Some(id) => {
            // The tree is read through this element: no probe timeout there.
            AXUIElementSetMessagingTimeout(element, 0.0);
            Some((element, id))
        }
        None => {
            CFRelease(element as CFTypeRef);
            None
        }
    }
}

/// A remote token: pid, four zero bytes, `'coco'`, then the element id.
fn remote_token(pid: i32, element_id: u64) -> [u8; 20] {
    let mut token = [0u8; 20];
    token[0..4].copy_from_slice(&pid.to_ne_bytes());
    token[8..12].copy_from_slice(&0x636f_636f_i32.to_ne_bytes());
    token[12..20].copy_from_slice(&element_id.to_ne_bytes());
    token
}

/// The element id in `element`'s remote token.
unsafe fn element_id(element: AXUIElementRef) -> Option<u64> {
    let data = _AXUIElementRemoteTokenCreate(element);
    if data.is_null() {
        return None;
    }
    let bytes = CFData::wrap_under_create_rule(data).bytes().to_vec();
    token_element_id(&bytes)
}

/// The element id of a remote token, if it is one.
fn token_element_id(token: &[u8]) -> Option<u64> {
    let id: [u8; 8] = token.get(12..20)?.try_into().ok()?;
    (token.len() == 20 && token[8..12] == 0x636f_636f_i32.to_ne_bytes())
        .then(|| u64::from_ne_bytes(id))
}

/// The element for remote token `element_id` in `pid`, retained.
unsafe fn element_from_token(pid: i32, element_id: u64) -> Option<AXUIElementRef> {
    let data = CFData::from_buffer(&remote_token(pid, element_id));
    let element = _AXUIElementCreateWithRemoteToken(data.as_concrete_TypeRef());
    (!element.is_null()).then_some(element)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_gives_back_its_element_id() {
        let token = remote_token(42, 319);
        assert_eq!(token_element_id(&token), Some(319));
        assert_eq!(token_element_id(&token[..12]), None);
        let mut app = token;
        app[8..12].copy_from_slice(b"appa");
        assert_eq!(token_element_id(&app), None);
    }

    #[test]
    fn a_remote_token_carries_the_pid_the_tag_and_the_element_id() {
        let token = remote_token(89504, 42);
        assert_eq!(&token[0..4], &89504_i32.to_ne_bytes());
        assert_eq!(&token[4..8], &[0, 0, 0, 0]);
        assert_eq!(&token[8..12], &0x636f_636f_i32.to_ne_bytes());
        assert_eq!(&token[12..20], &42_u64.to_ne_bytes());
    }

    #[test]
    fn only_a_whole_scan_rules_a_window_out() {
        let mut app = AppMemory::default();
        // Out of time at id 900: nothing is ruled out, the next scan goes on there.
        app.after_scan(0, Some(900), vec![7]);
        assert!(app.unmatched.is_empty());
        assert_eq!(app.resume, 900);
        // That scan goes through, but it began half way: still nothing ruled out,
        // and the next one starts from the first id.
        app.after_scan(900, None, vec![7]);
        assert!(app.unmatched.is_empty());
        assert_eq!(app.resume, 0);
        // A whole scan that finds no element for the window rules it out.
        app.after_scan(0, None, vec![7]);
        assert!(app.unmatched.contains(&7));
    }

    #[test]
    fn the_memory_starts_over_past_its_app_limit() {
        let mut memory = HashMap::new();
        for pid in 1..=MAX_APPS as i32 {
            app_memory(&mut memory, pid).found.insert(1, 1);
        }
        assert_eq!(memory.len(), MAX_APPS);
        // A known app does not clear it; a new one does.
        app_memory(&mut memory, 1);
        assert_eq!(memory.len(), MAX_APPS);
        app_memory(&mut memory, MAX_APPS as i32 + 1);
        assert_eq!(memory.len(), 1);
    }

    #[test]
    fn a_window_the_app_does_not_have_needs_no_scan() {
        // A pid with no windows in the window server list: the window is not
        // the app's, so no element is built and nothing is scanned.
        assert!(resolve(i32::MAX, 1).is_none());
        assert!(resolve(0, 1).is_none());
    }
}
