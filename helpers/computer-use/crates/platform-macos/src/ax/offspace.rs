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
//! [`windows_by_token`] finds the elements for the app's on-screen-list
//! windows that the regular attributes missed. Element ids are small and
//! stable for a window's life, so a found id is remembered, and a window that
//! a whole scan could not match (a hidden helper window, say) is not scanned
//! for again. A scan cut short by its time budget (a busy app) goes on where
//! it stopped next time instead.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use core_foundation::base::{CFRelease, CFTypeRef};

use super::bindings::{
    ax_get_window_id, copy_string_attr, element_from_token, AXUIElementRef,
    AXUIElementSetMessagingTimeout,
};

/// Element ids scanned for a missing window. Window elements sit far below
/// this in native and Electron apps alike.
const SCAN_LIMIT: u64 = 4096;
/// A scan stops here even if windows are still missing.
const SCAN_BUDGET: Duration = Duration::from_millis(400);
/// How long one probe may wait for the app; a missing id answers at once.
const PROBE_TIMEOUT_SECS: f32 = 0.1;
/// Apps remembered before the memory starts over.
const MAX_APPS: usize = 64;

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

/// The app's real windows (layer 0, not zero-sized) from the window server,
/// on any Space.
fn server_windows(pid: i32) -> Vec<u32> {
    crate::windows::all_windows()
        .into_iter()
        .filter(|w| w.pid == pid && w.layer == 0 && w.bounds.width > 1.0 && w.bounds.height > 1.0)
        .map(|w| w.window_id)
        .collect()
}

/// Retained window elements of `pid` for its windows not in `have`.
///
/// # Safety
/// Calls the AX API; each returned element must be released by the caller.
pub(crate) unsafe fn windows_by_token(pid: i32, have: &[u32]) -> Vec<AXUIElementRef> {
    let missing: Vec<u32> = server_windows(pid)
        .into_iter()
        .filter(|id| !have.contains(id))
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    let Ok(mut memory) = memory().lock() else {
        return Vec::new();
    };
    if memory.len() >= MAX_APPS && !memory.contains_key(&pid) {
        memory.clear();
    }
    let app = memory.entry(pid).or_default();
    let mut out = Vec::new();
    let mut still_missing = Vec::new();
    for id in missing {
        let remembered = app
            .found
            .get(&id)
            .and_then(|&element_id| window_element(pid, element_id, Some(id)));
        match remembered {
            Some((element, _)) => out.push(element),
            None => {
                app.found.remove(&id);
                still_missing.push(id);
            }
        }
    }
    if still_missing.iter().all(|id| app.unmatched.contains(id)) {
        return out;
    }
    let deadline = Instant::now() + SCAN_BUDGET;
    let started = app.resume;
    let mut stopped = None;
    for element_id in started..SCAN_LIMIT {
        if still_missing.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            stopped = Some(element_id);
            break;
        }
        let Some((element, window)) = window_element(pid, element_id, None) else {
            continue;
        };
        if let Some(at) = still_missing.iter().position(|&id| id == window) {
            still_missing.swap_remove(at);
            app.found.insert(window, element_id);
            app.unmatched.remove(&window);
            out.push(element);
        } else {
            CFRelease(element as CFTypeRef);
        }
    }
    app.after_scan(started, stopped, still_missing);
    out
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
            AXUIElementSetMessagingTimeout(element, 0.0);
            Some((element, id))
        }
        None => {
            CFRelease(element as CFTypeRef);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::bindings::remote_token;

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
        let mut app = super::AppMemory::default();
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
    fn an_app_with_no_windows_needs_no_scan() {
        // A pid with no windows in the window server list: nothing is missing,
        // so no element is built and nothing is scanned.
        let found = unsafe { super::windows_by_token(i32::MAX, &[]) };
        assert!(found.is_empty());
    }
}
