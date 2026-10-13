//! Whether the host pumps the AppKit main queue.
//!
//! Browser scripts run on the AppKit main thread. Work queued there from a
//! worker thread waits forever unless the host is running its main loop, so
//! the host says when it is, and a script fails at once until then.

use std::sync::atomic::{AtomicBool, Ordering};

static AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Call once the current process pumps the AppKit main queue.
pub fn mark_appkit_main_loop_available() {
    AVAILABLE.store(true, Ordering::Release);
}

/// Call when the main loop stops, or before it starts.
pub fn mark_appkit_main_loop_stopped() {
    AVAILABLE.store(false, Ordering::Release);
}

/// Whether work queued on the AppKit main queue will be serviced.
pub fn appkit_main_loop_available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}
