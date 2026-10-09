//! App quirks (Phase 5): apps whose structure isn't there until something asks for it.
//!
//! - **Windows on another Space nobody has touched.** Accessibility lists only the current
//!   Space's windows, and an element has a remote-token id only once a client reached it. Made
//!   key inside its own app by synthetic activation, the window is the app's focused window, which
//!   accessibility gives wherever it is; the defocus follows at once. The user's front app, key
//!   window and cursor don't change (measured 2026-10-09 on AppKit and SwiftUI windows, the user
//!   on a full-screen Space). It is never done to the user's own front app: making one of its
//!   windows key would take their typing.
//! - **Electron apps** build their accessibility tree only when a client sets
//!   `AXManualAccessibility` on the application element, and Electron delays the build by about
//!   2 s. It is set once per process instance (setting it again restarts the countdown), and an
//!   observe waits for the page's web area to fill, up to `READY_BOUND`.
//!
//! Browsers (Chromium-family apps by bundle id, `AXEnhancedUserInterface`) are the browser
//! stream's (`web.rs`), not this module's.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::CFBoolean;

use super::ax::AxEl;
use super::input::Activation;
use crate::desktop::{Structure, WindowInfo};
use crate::geom::Point;

/// How long an observe waits for a first-contact app to build its tree: Electron starts it about
/// 2 s after the attribute is set.
pub const READY_BOUND: Duration = Duration::from_millis(3500);
/// How deep under the window a web area is looked for.
const WEB_AREA_DEPTH: usize = 12;
/// How long a revealed window takes to become its app's focused window.
const REVEAL_WAIT: Duration = Duration::from_millis(300);

/// What first contact did to one process instance.
#[derive(Debug, Clone, Copy)]
struct Contact {
    start_us: u64,
    electron: bool,
    at: Instant,
}

/// Windows of Electron apps that have been key once (see `Quirks::wake`).
type Woken = HashSet<(i32, u64, u32)>;

#[derive(Default)]
pub struct Quirks {
    contacts: HashMap<i32, Contact>,
    woken: Woken,
}

impl Quirks {
    /// Makes first contact with `pid` once per process instance: an Electron app is asked for its
    /// tree. Cheap after the first call.
    pub fn first_contact(&mut self, pid: i32) -> bool {
        let start_us = super::process_start_us(pid).unwrap_or(0);
        if let Some(c) = self.contacts.get(&pid)
            && c.start_us == start_us
        {
            return c.electron;
        }
        let electron = is_electron(pid);
        if electron {
            let _ = AxEl::app(pid).set("AXManualAccessibility", CFBoolean::new(true));
        }
        self.contacts.insert(
            pid,
            Contact {
                start_us,
                electron,
                at: Instant::now(),
            },
        );
        electron
    }

    /// Makes an Electron window key inside its app once, by synthetic activation, then lets it go.
    /// Until a window has been key, Electron serves no page tree for it or leaves its presses
    /// unanswered (measured 2026-10-09: an untouched fixture window gave no web area for 8 s, or
    /// a tree whose presses did nothing; after one activation, both worked from then on). Not
    /// done to the user's own front app, which is active already.
    pub fn wake(&mut self, w: &WindowInfo, user_front: Option<i32>) {
        if !self.first_contact(w.pid) || user_front == Some(w.pid) {
            return;
        }
        let start_us = self.contacts.get(&w.pid).map_or(0, |c| c.start_us);
        if !self.woken.insert((w.pid, start_us, w.id)) {
            return;
        }
        drop(Activation::begin(w.pid, w.id, true));
    }

    /// Whether `w`'s structure is complete: an Electron window is pending while its web area is
    /// missing or empty, until `READY_BOUND` after first contact.
    pub fn structure(&mut self, w: &WindowInfo, window: Option<&AxEl>) -> Structure {
        if !self.first_contact(w.pid) {
            return Structure::Ready;
        }
        let Some(window) = window else {
            return Structure::Ready;
        };
        if web_area_filled(window) {
            return Structure::Ready;
        }
        let age = self.contacts.get(&w.pid).map(|c| c.at.elapsed());
        if age.is_some_and(|a| a < READY_BOUND) {
            Structure::Pending
        } else {
            Structure::Incomplete
        }
    }
}

/// Electron apps carry Electron's framework in their bundle.
fn is_electron(pid: i32) -> bool {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return false;
    };
    let Some(path) = app
        .bundleURL()
        .and_then(|u| u.path())
        .map(|p| p.to_string())
    else {
        return false;
    };
    Path::new(&path)
        .join("Contents/Frameworks/Electron Framework.framework")
        .exists()
}

/// A web area with at least one child, breadth first. Electron nests it seven or so groups
/// under the window.
fn web_area_filled(window: &AxEl) -> bool {
    let mut level = vec![window.clone()];
    for _ in 0..WEB_AREA_DEPTH {
        let mut next = Vec::new();
        for el in level {
            if el.string("AXRole").as_deref() == Some("AXWebArea") {
                return !el.elements("AXChildren").is_empty();
            }
            next.extend(el.elements("AXChildren"));
        }
        if next.is_empty() {
            return false;
        }
        level = next;
    }
    false
}

/// The point element frames are made relative to: the window's top left as accessibility gives
/// it, so they are window points wherever the window is. Accessibility places a window on
/// another Space whole display widths away from where the window server has it (measured
/// 2026-10-09: x 3528 for a window the window server had at 72, on a 1728 pt display), so the
/// window server's origin would put every element of such a window thousands of points off.
pub fn origin(w: &WindowInfo, window: Option<&AxEl>) -> Point {
    window
        .and_then(AxEl::position)
        .unwrap_or(Point::new(w.frame.x, w.frame.y))
}

/// A window on another Space that accessibility doesn't list: made key inside its app, read as
/// the app's focused window, then defocused. `None` when the app is the user's front app, or
/// the window doesn't become key (minimised, ordered out, or an app that refuses activation).
pub fn reveal(w: &WindowInfo, user_front: Option<i32>) -> Option<AxEl> {
    if w.on_screen || w.minimized || w.hidden || user_front == Some(w.pid) {
        return None;
    }
    let act = Activation::begin(w.pid, w.id, true).ok()?;
    let app = AxEl::app(w.pid);
    let end = Instant::now() + REVEAL_WAIT;
    let mut found = None;
    while Instant::now() < end {
        if let Some(el) = app.element("AXFocusedWindow")
            && el.window_id() == Some(w.id)
        {
            found = Some(el);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(act);
    found
}
