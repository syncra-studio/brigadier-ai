//! The AppKit main loop the daemon, `mcp` and one-shot `call` run on the main
//! thread while their worker runs elsewhere.
//!
//! Browser automation and the permissions panel dispatch work to the AppKit
//! main queue. That work waits forever unless main pumps the queue, so the
//! loop says when it is available, and a stop request is latched so a worker
//! that finishes before the loop starts still ends it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

static APPKIT_LOOP_AVAILABLE: AtomicBool = AtomicBool::new(false);
static APPKIT_LOOP_RUNNING: AtomicBool = AtomicBool::new(false);
static APPKIT_STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Reset completion state before spawning a worker that will be paired with an
/// AppKit main loop. This does not advertise the loop yet, so work received
/// during setup or the permissions gate fails fast instead of queueing forever.
pub fn prepare_appkit_main_loop() {
    APPKIT_STOP_REQUESTED.store(false, Ordering::Release);
    APPKIT_LOOP_RUNNING.store(false, Ordering::Release);
    APPKIT_LOOP_AVAILABLE.store(false, Ordering::Release);
    // The old driver's cursor overlay loop reads its own flags until the switch.
    platform_macos::pip::prepare_appkit_main_loop();
}

/// Advertise that the current process is about to pump the AppKit main queue.
pub fn mark_appkit_main_loop_available() {
    APPKIT_LOOP_AVAILABLE.store(true, Ordering::Release);
    // The old driver's browser automation reads its own flag until the switch.
    platform_macos::pip::mark_appkit_main_loop_available();
}

/// Whether worker-thread AppKit dispatch is guaranteed to be serviced.
pub fn appkit_main_loop_available() -> bool {
    APPKIT_LOOP_AVAILABLE.load(Ordering::Acquire)
}

pub fn appkit_main_loop_stop_requested() -> bool {
    APPKIT_STOP_REQUESTED.load(Ordering::Acquire)
}

fn mark_appkit_main_loop_running() {
    APPKIT_LOOP_RUNNING.store(true, Ordering::Release);
}

fn mark_appkit_main_loop_stopped() {
    APPKIT_LOOP_RUNNING.store(false, Ordering::Release);
    APPKIT_LOOP_AVAILABLE.store(false, Ordering::Release);
}

/// Ask the paired AppKit loop to return. Safe before the loop starts: the
/// request is latched and `run_appkit_main_loop` skips its event pump.
pub fn request_appkit_main_loop_stop() {
    // Close the dispatch gate before latching shutdown so a concurrent browser
    // request cannot enter a queue that this loop is about to abandon.
    APPKIT_LOOP_AVAILABLE.store(false, Ordering::Release);
    // The old driver's browser automation and cursor overlay loop read its
    // own flags until the switch; this also stops the overlay's loop.
    platform_macos::pip::request_appkit_main_loop_stop();
    APPKIT_STOP_REQUESTED.store(true, Ordering::Release);
    if APPKIT_LOOP_RUNNING.load(Ordering::Acquire) {
        // CFRunLoopStop/WakeUp are explicitly cross-thread APIs. The main
        // event pump observes STOP_REQUESTED immediately after waking.
        unsafe {
            let run_loop = CFRunLoopGetMain();
            CFRunLoopStop(run_loop);
            CFRunLoopWakeUp(run_loop);
        }
    }
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopStop(run_loop: *mut c_void);
    fn CFRunLoopWakeUp(run_loop: *mut c_void);
}

/// Pump AppKit events on the main thread. Used by macOS Call, Serve, and MCP
/// when the cursor overlay is absent so browser automation dispatches to the
/// main queue are drained. Mirrors the cursor overlay's startup without
/// installing the overlay's CALayer-backed window itself.
///
/// Returns after [`request_appkit_main_loop_stop`] is serviced.
pub fn run_appkit_main_loop() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};

    let _mtm = objc2_foundation::MainThreadMarker::new()
        .expect("run_appkit_main_loop must be called from the main thread");
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        // Accessory policy: no Dock icon, no menu bar. Keeps the
        // daemon out of the user's application switcher, same as
        // the cursor overlay's NSApp setup.
        let _: bool = msg_send![app, setActivationPolicy: 1i64];
        let _: () = msg_send![app, finishLaunching];
        if !appkit_main_loop_stop_requested() {
            mark_appkit_main_loop_available();
            mark_appkit_main_loop_running();
            run_appkit_event_loop(app);
        }
        mark_appkit_main_loop_stopped();
    }
}

/// Pump AppKit events and the main CFRunLoop until the paired worker requests
/// shutdown. A bounded `nextEvent` wait makes completion observable even if a
/// platform wakeup is lost, while still routing normal cursor events.
unsafe fn run_appkit_event_loop(app: *mut objc2::runtime::AnyObject) {
    use objc2::rc::autoreleasepool;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;

    let mode = NSString::from_str("kCFRunLoopDefaultMode");
    while !appkit_main_loop_stop_requested() {
        autoreleasepool(|_| unsafe {
            let until: *mut AnyObject = msg_send![
                class!(NSDate),
                dateWithTimeIntervalSinceNow: 1.0f64
            ];
            let event: *mut AnyObject = msg_send![app,
                nextEventMatchingMask: u64::MAX
                untilDate: until
                inMode: &*mode
                dequeue: true
            ];
            if !event.is_null() {
                let _: () = msg_send![app, sendEvent: event];
            }
            let _: () = msg_send![app, updateWindows];
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::run_guarded_appkit_worker;

    #[test]
    fn worker_panic_becomes_failure_status_instead_of_stranding_main_loop() {
        assert_eq!(run_guarded_appkit_worker("test", || {}), 0);
        super::prepare_appkit_main_loop();
        super::mark_appkit_main_loop_available();
        let worker = std::thread::spawn(|| {
            let status = run_guarded_appkit_worker("test", || panic!("worker failed"));
            super::request_appkit_main_loop_stop();
            status
        });
        assert_eq!(worker.join().unwrap(), 101);
        assert!(super::appkit_main_loop_stop_requested());
        assert!(!super::appkit_main_loop_available());
        // The old driver's flags follow until the switch.
        assert!(platform_macos::pip::appkit_main_loop_stop_requested());
        assert!(!platform_macos::pip::appkit_main_loop_available());
    }
}
