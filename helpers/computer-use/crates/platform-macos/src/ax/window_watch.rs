//! Hears about an app's windows as they open or become main.
//!
//! A window on another Space can only be reached once the app has handed it
//! out (see [`super::offspace`]), and the app hands it out only while it is
//! the main or focused window. When several documents open at once, each is
//! main for a moment only, too briefly to be caught by looking. Watching the
//! app's window notifications catches every one: the element each
//! notification carries is the window itself, already handed out, so it is
//! remembered on the spot.
//!
//! The notifications arrive on a thread of their own, so a busy main thread
//! can't delay them.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::runloop::{
    kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopGetCurrent,
    CFRunLoopRef, CFRunLoopRemoveSource, CFRunLoopRunInMode, CFRunLoopSourceRef, CFRunLoopWakeUp,
};
use core_foundation::string::{CFString, CFStringRef};

use super::bindings::{
    copy_element_attr, copy_string_attr, AXError, AXUIElementCreateApplication, AXUIElementGetPid,
    AXUIElementRef,
};

type AXObserverRef = *mut c_void;
type AXObserverCallback =
    unsafe extern "C" fn(AXObserverRef, AXUIElementRef, CFStringRef, *mut c_void);

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXObserverCreate(
        application: i32,
        callback: AXObserverCallback,
        out_observer: *mut AXObserverRef,
    ) -> AXError;
    fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
}

/// Window notifications that hand out the window they are about.
const NOTIFICATIONS: [&str; 3] = [
    "AXWindowCreated",
    "AXMainWindowChanged",
    "AXFocusedWindowChanged",
];
/// Apps watched before the ones that have quit are let go.
const MAX_APPS: usize = 64;

/// The run loop of the thread the notifications arrive on.
struct Loop(CFRunLoopRef);
// SAFETY: a run loop reference may be used from any thread to add or remove
// sources and wake it.
unsafe impl Send for Loop {}
unsafe impl Sync for Loop {}

/// A watched app's observer (retained).
struct Observer(AXObserverRef);
// SAFETY: only added to and removed from the watch thread's run loop, which
// both take any thread.
unsafe impl Send for Observer {}

fn watch_loop() -> &'static Loop {
    static LOOP: OnceLock<Loop> = OnceLock::new();
    LOOP.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("window-watch".into())
            .spawn(move || unsafe {
                let _ = tx.send(Loop(CFRunLoopGetCurrent()));
                loop {
                    // Returns at once while nothing is watched yet.
                    if CFRunLoopRunInMode(kCFRunLoopDefaultMode, 60.0, 0) == 1 {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                    }
                }
            })
            .expect("window-watch thread");
        rx.recv().expect("window-watch run loop")
    })
}

fn watched() -> &'static Mutex<HashMap<i32, Observer>> {
    static WATCHED: OnceLock<Mutex<HashMap<i32, Observer>>> = OnceLock::new();
    WATCHED.get_or_init(Default::default)
}

unsafe extern "C" fn on_window(
    _observer: AXObserverRef,
    element: AXUIElementRef,
    _notification: CFStringRef,
    _refcon: *mut c_void,
) {
    let mut pid = 0;
    if element.is_null() || AXUIElementGetPid(element, &mut pid) != 0 {
        return;
    }
    if copy_string_attr(element, "AXRole").as_deref() == Some("AXWindow") {
        super::offspace::remember(pid, element);
    } else if let Some(window) = copy_element_attr(element, "AXWindow") {
        super::offspace::remember(pid, window);
        CFRelease(window as CFTypeRef);
    }
}

fn alive(pid: i32) -> bool {
    let signalled = unsafe { libc::kill(pid, 0) } == 0;
    signalled || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

/// Starts watching `pid`'s windows, once per app.
pub(crate) fn watch(pid: i32) {
    if pid <= 0 {
        return;
    }
    let Ok(mut watched) = watched().lock() else {
        return;
    };
    if watched.contains_key(&pid) {
        return;
    }
    let run_loop = watch_loop().0;
    if watched.len() >= MAX_APPS {
        watched.retain(|&p, observer| {
            let keep = alive(p);
            if !keep {
                unsafe { forget(run_loop, observer.0) };
            }
            keep
        });
    }
    unsafe {
        let mut observer: AXObserverRef = std::ptr::null_mut();
        if AXObserverCreate(pid, on_window, &mut observer) != 0 || observer.is_null() {
            return;
        }
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            CFRelease(observer as CFTypeRef);
            return;
        }
        let mut added = 0;
        for name in NOTIFICATIONS {
            let name = CFString::new(name);
            if AXObserverAddNotification(
                observer,
                app,
                name.as_concrete_TypeRef(),
                std::ptr::null_mut(),
            ) == 0
            {
                added += 1;
            }
        }
        CFRelease(app as CFTypeRef);
        if added == 0 {
            // An app that can't be watched (not yet ready, or no AX) is tried
            // again on its next touch.
            CFRelease(observer as CFTypeRef);
            return;
        }
        CFRunLoopAddSource(
            run_loop,
            AXObserverGetRunLoopSource(observer),
            kCFRunLoopCommonModes,
        );
        CFRunLoopWakeUp(run_loop);
        watched.insert(pid, Observer(observer));
    }
}

unsafe fn forget(run_loop: CFRunLoopRef, observer: AXObserverRef) {
    CFRunLoopRemoveSource(
        run_loop,
        AXObserverGetRunLoopSource(observer),
        kCFRunLoopCommonModes,
    );
    CFRelease(observer as CFTypeRef);
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_process_that_does_not_exist_is_not_alive() {
        assert!(super::alive(std::process::id() as i32));
        assert!(!super::alive(i32::MAX));
    }

    #[test]
    fn nothing_is_watched_for_no_app() {
        super::watch(0);
        super::watch(-1);
        let watched = super::watched().lock().unwrap();
        assert!(!watched.contains_key(&0) && !watched.contains_key(&-1));
    }
}
