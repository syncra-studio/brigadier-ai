//! Keep a background dialog's owner under the user's window while the dialog closes.
//!
//! When a modal dialog ends (Cancel, Save, its Close button), Windows
//! activates the dialog's owner inside the app's own thread. That activation
//! does not take the foreground, but it raises the owner in z-order above the
//! user's front window until something puts it back. Live (2026-11-02,
//! Windows 11 26300): Notepad's Save as Cancel raised Notepad above the user's
//! window for 80 ms, with WS_EX_NOACTIVATE set on the owner. A hidden window
//! cannot cover anything, so the owner is hidden for the action and shown
//! again directly under the front window without activation.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, SetWindowPos, GA_ROOT, GA_ROOTOWNER, HWND_BOTTOM, SWP_HIDEWINDOW,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SWP_SHOWWINDOW,
};

/// Facts about a click target window and the front window.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OwnerFacts {
    /// The click target is a classic dialog (#32770) top-level window.
    pub classic_dialog: bool,
    /// The dialog has a root owner other than itself.
    pub owned: bool,
    /// The owner belongs to the same process as the dialog.
    pub same_process: bool,
    pub owner_visible: bool,
    pub owner_iconic: bool,
    /// There is a front window, and it is neither the dialog nor its owner.
    pub other_window_in_front: bool,
}

/// Hide the owner only for a background dialog of the target app itself:
/// never a window of the user's, never one the user is working in.
pub(crate) fn hide_owner(facts: OwnerFacts) -> bool {
    facts.classic_dialog
        && facts.owned
        && facts.same_process
        && facts.owner_visible
        && !facts.owner_iconic
        && facts.other_window_in_front
}

fn class_is_dialog(hwnd: HWND) -> bool {
    let mut class = [0u16; 16];
    let len = unsafe { GetClassNameW(hwnd, &mut class) };
    len > 0 && String::from_utf16_lossy(&class[..len as usize]) == "#32770"
}

fn process(hwnd: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

/// Owners hidden by clicks that have not shown them yet. A click whose
/// provider call hangs never reaches its drop, so the caller that gave up at
/// the deadline shows them through [`restore_hidden_owners`].
#[derive(Debug, Default)]
struct OwnerRegistry {
    hidden: Vec<isize>,
}

impl OwnerRegistry {
    /// Hide through `hide` only while the click is still wanted, and record
    /// what it hid. The deadline marks the click cancelled before its caller
    /// drains, so a late arm never hides a window nobody would show again.
    fn admit(
        &mut self,
        cancelled: &AtomicBool,
        hide: impl FnOnce() -> Option<isize>,
    ) -> Option<isize> {
        if cancelled.load(Ordering::Acquire) {
            return None;
        }
        let owner = hide()?;
        self.hidden.push(owner);
        Some(owner)
    }

    /// True when `owner` was still recorded, so this caller must show it.
    fn release(&mut self, owner: isize) -> bool {
        let before = self.hidden.len();
        self.hidden.retain(|hidden| *hidden != owner);
        self.hidden.len() != before
    }

    fn drain(&mut self) -> Vec<isize> {
        std::mem::take(&mut self.hidden)
    }
}

static HIDDEN_OWNERS: Mutex<OwnerRegistry> = Mutex::new(OwnerRegistry { hidden: Vec::new() });

fn registry() -> std::sync::MutexGuard<'static, OwnerRegistry> {
    HIDDEN_OWNERS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Show every owner a click hid and has not shown yet. Called when a click
/// timed out: its worker may stay blocked in the provider indefinitely.
pub(crate) fn restore_hidden_owners() {
    let owners = registry().drain();
    for owner in owners {
        show_under_front(HWND(owner as *mut _));
    }
}

/// Show the owner again directly under the front window, without activation.
fn show_under_front(owner: HWND) {
    if !unsafe { IsWindow(owner) }.as_bool() {
        return;
    }
    let front = unsafe { GetForegroundWindow() };
    let after = if front.0.is_null() || front == owner {
        HWND_BOTTOM
    } else {
        unsafe { GetAncestor(front, GA_ROOT) }
    };
    let flags = SWP_SHOWWINDOW | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE;
    if unsafe { SetWindowPos(owner, after, 0, 0, 0, 0, flags) }.is_err() {
        // Never leave the app's window hidden.
        let _ = unsafe { SetWindowPos(owner, HWND_BOTTOM, 0, 0, 0, 0, flags) };
    }
}

/// Hides the owner on creation and shows it under the front window on drop,
/// unless the caller that gave up on the click has shown it already.
pub(crate) struct HiddenOwner(Option<HWND>);

impl HiddenOwner {
    pub(crate) fn arm(dialog: HWND, cancelled: &AtomicBool) -> Self {
        let root = unsafe { GetAncestor(dialog, GA_ROOT) };
        let dialog = if root.0.is_null() { dialog } else { root };
        let owner = unsafe { GetAncestor(dialog, GA_ROOTOWNER) };
        let front = unsafe { GetForegroundWindow() };
        let facts = OwnerFacts {
            classic_dialog: class_is_dialog(dialog),
            owned: !owner.0.is_null() && owner != dialog,
            same_process: process(dialog) != 0 && process(dialog) == process(owner),
            owner_visible: unsafe { IsWindowVisible(owner) }.as_bool(),
            owner_iconic: unsafe { IsIconic(owner) }.as_bool(),
            other_window_in_front: !front.0.is_null()
                && front != dialog
                && unsafe { GetAncestor(front, GA_ROOTOWNER) } != owner,
        };
        if !hide_owner(facts) {
            return Self(None);
        }
        let flags = SWP_HIDEWINDOW
            | SWP_NOACTIVATE
            | SWP_NOMOVE
            | SWP_NOSIZE
            | SWP_NOZORDER
            | SWP_NOOWNERZORDER;
        // Hide under the registry lock, so a caller draining at the deadline
        // sees either nothing hidden or this owner recorded.
        let hidden = registry().admit(cancelled, || {
            let hidden = unsafe { SetWindowPos(owner, None, 0, 0, 0, 0, flags) }.is_ok()
                && !unsafe { IsWindowVisible(owner) }.as_bool();
            hidden.then_some(owner.0 as isize)
        });
        Self(hidden.map(|_| owner))
    }
}

impl Drop for HiddenOwner {
    fn drop(&mut self) {
        let Some(owner) = self.0.take() else {
            return;
        };
        // Bind first: show_under_front must not run under the registry lock.
        let still_hidden = registry().release(owner.0 as isize);
        if still_hidden {
            show_under_front(owner);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn background_dialog() -> OwnerFacts {
        OwnerFacts {
            classic_dialog: true,
            owned: true,
            same_process: true,
            owner_visible: true,
            owner_iconic: false,
            other_window_in_front: true,
        }
    }

    #[test]
    fn only_a_background_dialogs_own_visible_owner_is_hidden() {
        assert!(hide_owner(background_dialog()));
        for facts in [
            OwnerFacts {
                classic_dialog: false,
                ..background_dialog()
            },
            OwnerFacts {
                owned: false,
                ..background_dialog()
            },
            // A dialog owned by another process's window, such as the user's.
            OwnerFacts {
                same_process: false,
                ..background_dialog()
            },
            OwnerFacts {
                owner_visible: false,
                ..background_dialog()
            },
            OwnerFacts {
                owner_iconic: true,
                ..background_dialog()
            },
            // The user is working in the dialog or its owner.
            OwnerFacts {
                other_window_in_front: false,
                ..background_dialog()
            },
        ] {
            assert!(!hide_owner(facts), "{facts:?}");
        }
    }

    #[test]
    fn a_window_that_is_not_an_owned_dialog_is_never_hidden() {
        assert!(HiddenOwner::arm(HWND::default(), &AtomicBool::new(false))
            .0
            .is_none());
    }

    #[test]
    fn a_cancelled_click_hides_nothing() {
        let mut registry = OwnerRegistry::default();
        let mut hid = false;
        let admitted = registry.admit(&AtomicBool::new(true), || {
            hid = true;
            Some(7)
        });
        assert_eq!(admitted, None);
        assert!(!hid);
        assert!(registry.drain().is_empty());
    }

    #[test]
    fn an_owner_hidden_by_a_hung_click_is_shown_once_by_the_caller() {
        let mut registry = OwnerRegistry::default();
        assert_eq!(registry.admit(&AtomicBool::new(false), || Some(7)), Some(7));
        // The caller gave up at the deadline and shows what is still hidden.
        assert_eq!(registry.drain(), vec![7]);
        // The worker returning late must not show it a second time.
        assert!(!registry.release(7));
    }

    #[test]
    fn a_click_that_finishes_shows_its_own_owner() {
        let mut registry = OwnerRegistry::default();
        assert_eq!(registry.admit(&AtomicBool::new(false), || Some(7)), Some(7));
        assert!(registry.release(7));
        assert!(registry.drain().is_empty());
    }
}
