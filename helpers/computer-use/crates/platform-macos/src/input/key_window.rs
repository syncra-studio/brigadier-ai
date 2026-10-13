//! Keystrokes posted to an app land in whatever holds its keyboard focus.
//!
//! An app in the background still has a key window and a focused element,
//! and posted key events (and writes to "the focused element") go there, not
//! to the window the agent named. With two agents in two documents of one
//! app, typing into one document would land in the other one and still look
//! like a success. [`ensure_in`] makes sure the named window holds the
//! keyboard focus before anything is typed: it asks the app to make that
//! window its main window (the app is not brought to the front), and refuses
//! when the focus stays elsewhere. [`lease`] keeps two agents typing into one
//! app from taking turns at the focus in the middle of each other's text.

use std::collections::HashSet;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::CFString;

use crate::ax::bindings::{
    ax_get_window_id, copy_ax_windows, copy_element_attr, copy_string_attr,
    kAXErrorAttributeUnsupported, kAXErrorNoValue, kAXErrorSuccess, set_bool_attr_true, AXError,
    AXUIElementCopyAttributeValue, AXUIElementCreateApplication, AXUIElementGetTypeID,
    AXUIElementRef, AXUIElementSetMessagingTimeout,
};

/// How many times the named window is made main before giving up.
const ATTEMPTS: usize = 3;
/// How long the app gets to move its focus after each attempt.
const SETTLE: Duration = Duration::from_millis(30);
/// Enough to climb from any field to its window.
const MAX_DEPTH: usize = 64;

fn busy() -> &'static (Mutex<HashSet<i32>>, Condvar) {
    static BUSY: OnceLock<(Mutex<HashSet<i32>>, Condvar)> = OnceLock::new();
    BUSY.get_or_init(Default::default)
}

/// Exclusive keyboard use of one app, held while one call types into it.
pub(crate) struct KeyLease(i32);

impl Drop for KeyLease {
    fn drop(&mut self) {
        let (lock, freed) = busy();
        if let Ok(mut held) = lock.lock() {
            held.remove(&self.0);
        }
        freed.notify_all();
    }
}

/// Waits until no other call is typing into `pid`, then holds it.
pub(crate) fn lease(pid: i32) -> KeyLease {
    let (lock, freed) = busy();
    let mut held = lock.lock().unwrap_or_else(|e| e.into_inner());
    while held.contains(&pid) {
        held = freed.wait(held).unwrap_or_else(|e| e.into_inner());
    }
    held.insert(pid);
    KeyLease(pid)
}

/// The windows holding `element`: its own window and every window or sheet
/// above it, so a field in a sheet counts as being in the sheet's window.
///
/// # Safety
/// `element` must be a valid AX element.
unsafe fn windows_holding(element: AXUIElementRef) -> Vec<u32> {
    let mut ids: Vec<u32> = ax_get_window_id(element).into_iter().collect();
    let mut current = element;
    let mut owned = false;
    for _ in 0..MAX_DEPTH {
        let role = copy_string_attr(current, "AXRole").unwrap_or_default();
        if matches!(role.as_str(), "AXWindow" | "AXSheet" | "AXDrawer") {
            ids.extend(ax_get_window_id(current));
        }
        let parent = copy_element_attr(current, "AXParent");
        if owned {
            CFRelease(current as CFTypeRef);
        }
        match parent {
            Some(p) => {
                current = p;
                owned = true;
            }
            None => return ids,
        }
    }
    if owned {
        CFRelease(current as CFTypeRef);
    }
    ids
}

/// How long the app gets to say where its focus is.
const ANSWER_SECS: f32 = 1.0;

/// The element `attr` of `element`: `Ok(None)` when the app has none,
/// `Err` when it didn't answer.
///
/// # Safety
/// `element` must be a valid AX element.
unsafe fn copy_focus_attr(
    element: AXUIElementRef,
    attr: &str,
) -> Result<Option<AXUIElementRef>, AXError> {
    let name = CFString::new(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let error = AXUIElementCopyAttributeValue(element, name.as_concrete_TypeRef(), &mut value);
    if error == kAXErrorSuccess && !value.is_null() {
        if core_foundation::base::CFGetTypeID(value) == AXUIElementGetTypeID() {
            return Ok(Some(value as AXUIElementRef));
        }
        CFRelease(value);
        return Ok(None);
    }
    if [
        kAXErrorSuccess,
        kAXErrorNoValue,
        kAXErrorAttributeUnsupported,
    ]
    .contains(&error)
    {
        Ok(None)
    } else {
        Err(error)
    }
}

/// Where keystrokes posted to `pid` land now: the windows holding its
/// focused element, else its focused window. Empty when the app reports
/// neither; `Err` when it didn't answer, so where they land is unknown.
fn focus_windows(pid: i32) -> Result<Vec<u32>, AXError> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Ok(Vec::new());
        }
        AXUIElementSetMessagingTimeout(app, ANSWER_SECS);
        let found = (|| {
            if let Some(focused) = copy_focus_attr(app, "AXFocusedUIElement")? {
                AXUIElementSetMessagingTimeout(focused, ANSWER_SECS);
                let ids = windows_holding(focused);
                CFRelease(focused as CFTypeRef);
                if !ids.is_empty() {
                    return Ok(ids);
                }
            }
            Ok(match copy_focus_attr(app, "AXFocusedWindow")? {
                Some(window) => {
                    let ids = ax_get_window_id(window).into_iter().collect();
                    CFRelease(window as CFTypeRef);
                    ids
                }
                None => Vec::new(),
            })
        })();
        CFRelease(app as CFTypeRef);
        found
    }
}

/// Asks the app to make `window_id` its main window and, when given, focus
/// `element` in it. Nothing is brought to the front.
fn make_main(pid: i32, window_id: u32, element: Option<usize>) {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return;
        }
        for window in copy_ax_windows(app) {
            if ax_get_window_id(window) == Some(window_id) {
                set_bool_attr_true(window, "AXMain");
                set_bool_attr_true(window, "AXFocused");
            }
            CFRelease(window as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
        if let Some(ptr) = element {
            set_bool_attr_true(ptr as AXUIElementRef, "AXFocused");
        }
    }
}

/// Whether keystrokes for `window_id` may be posted, given the windows that
/// hold the app's focus. An app that reports no focus can't send them
/// anywhere else.
fn focus_is_in(focus: &[u32], window_id: u32) -> bool {
    focus.is_empty() || focus.contains(&window_id)
}

/// Makes sure keystrokes posted to `pid` reach `window_id` (and `element` in
/// it, when given), or fails without typing anything.
pub(crate) fn ensure_in(pid: i32, window_id: u32, element: Option<usize>) -> anyhow::Result<()> {
    let unanswered = |error: AXError| {
        anyhow::anyhow!(
            "the app did not answer when asked which of its windows has the keyboard focus \
             (error {error}), so the keys could land in another window. Nothing was sent. \
             Take a fresh get_window_state, then try again."
        )
    };
    let mut focus = focus_windows(pid).map_err(unanswered)?;
    if focus_is_in(&focus, window_id) {
        return Ok(());
    }
    for _ in 0..ATTEMPTS {
        make_main(pid, window_id, element);
        std::thread::sleep(SETTLE);
        focus = focus_windows(pid).map_err(unanswered)?;
        if focus_is_in(&focus, window_id) {
            return Ok(());
        }
    }
    let elsewhere = focus
        .first()
        .map(|id| format!(" (window {id})"))
        .unwrap_or_default();
    anyhow::bail!(
        "the app's keyboard focus is in another of its windows{elsewhere}, not window \
         {window_id}, and it would not move, so the keys would land there. Nothing was \
         sent. Call get_window_state for window {window_id} and pass the field's \
         element_index, or use delivery_mode:\"foreground\"."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_in_the_named_window_or_a_window_above_it_counts() {
        assert!(focus_is_in(&[7], 7));
        assert!(focus_is_in(&[9, 7], 7));
        assert!(!focus_is_in(&[9], 7));
    }

    #[test]
    fn an_app_without_focus_does_not_block() {
        assert!(focus_is_in(&[], 7));
    }

    #[test]
    fn a_lease_waits_for_the_one_before_it() {
        let first = lease(-42);
        let (tx, rx) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _second = lease(-42);
            tx.send(()).unwrap();
        });
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        drop(first);
        assert!(rx.recv_timeout(Duration::from_secs(2)).is_ok());
        waiter.join().unwrap();
    }
}
