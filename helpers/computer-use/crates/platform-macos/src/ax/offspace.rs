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
//! a scan could not match (a hidden helper window, say) is not scanned for
//! again.

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
    for element_id in 0..SCAN_LIMIT {
        if still_missing.is_empty() || Instant::now() >= deadline {
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
    app.unmatched.extend(still_missing);
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
    fn an_app_with_no_windows_needs_no_scan() {
        // A pid with no windows in the window server list: nothing is missing,
        // so no element is built and nothing is scanned.
        let found = unsafe { super::windows_by_token(i32::MAX, &[]) };
        assert!(found.is_empty());
    }
}
