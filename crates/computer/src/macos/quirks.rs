//! App quirks (Phase 5): apps whose structure isn't there until something asks for it.
//!
//! - **Windows on another Space nobody has touched.** Accessibility lists only the current
//!   Space's windows, and an element has a remote-token id only once a client reached it. Made
//!   key inside its own app by synthetic activation, the window is the app's focused window, which
//!   accessibility gives wherever it is; the defocus follows at once. The user's front app, key
//!   window and cursor don't change (measured 2026-10-09 on AppKit and SwiftUI windows, the user
//!   on a full-screen Space). It is never done to the user's own front app: making one of its
//!   windows key would take their typing.
//! - **First contact**, once per process instance, on the application element. Some apps build
//!   their tree only when a client asks for it: Electron when `AXManualAccessibility` is set,
//!   Chromium browsers when `AXEnhancedUserInterface` is (both flags are set on them, and stay
//!   set for the process), each about 2 s later; setting a flag again restarts that countdown.
//!   WebKit fills a page's tree lazily once it is read, and may first report its scroll area
//!   outside the window. AppKit, SwiftUI and Catalyst windows add elements in the moments after
//!   the first query (measured 2026-10-09: a Catalyst window 19 → 20 elements and an AppKit one
//!   36 → 38 within 150 ms; a Catalyst stepper came with the app's launch, later still).
//! - **One settle per window.** The first observe of a window walks it until its element count
//!   holds for `SETTLE_QUIET` and the app is past `LAUNCHING`: the page's elements where the
//!   window holds a page (or will: Electron and Chromium windows always do), every element
//!   otherwise. A page also has to have content inside its window. It waits up to `PAGE_BOUND`
//!   for a page and `SETTLE_BOUND` for anything else, once per window and process instance. An
//!   app already running pays one quiet window; only an app launched a moment ago, or a page
//!   still being built, waits longer.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use objc2_app_kit::NSRunningApplication;
use objc2_core_foundation::CFBoolean;

use super::ax::AxEl;
use super::input::Activation;
use super::web;
use crate::desktop::{Structure, WindowInfo};
use crate::geom::Point;

/// How long a window's first observe waits for its page: Electron and Chromium start building it
/// about 2 s after their flag is set, and a busy Mac adds to that. Under the engine's own wait.
const PAGE_BOUND: Duration = Duration::from_millis(4500);
/// How long an empty view (`web::empty_host`) is given to show the page it will hold.
const APPEAR_WAIT: Duration = Duration::from_secs(1);
/// How long a window's element count must hold on first contact before its tree counts as built.
/// 100 ms was too short under load (2026-10-09, load average 15: a Catalyst stepper came later).
const SETTLE_QUIET: Duration = Duration::from_millis(250);
/// How long an app counts as launching: it may build controls with no read to prompt it, after a
/// quiet second (measured 2026-10-09: a Catalyst stepper 1.1–1.2 s after launch in 5 of 8
/// launches, the count unchanged for the second before). A window of an app this young is
/// settled no sooner than this after its process started.
const LAUNCHING: Duration = Duration::from_millis(2000);
/// The longest first-contact settle of a window without a page: an app that keeps changing (a
/// clock, a progress bar) is taken as it is.
const SETTLE_BOUND: Duration = Duration::from_millis(2500);
/// How long a revealed window takes to become its app's focused window.
const REVEAL_WAIT: Duration = Duration::from_millis(300);

/// What an app is asked for on first contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Native,
    /// `AXManualAccessibility`; its windows are made key once (`Quirks::wake`).
    Electron,
    /// `AXManualAccessibility` and `AXEnhancedUserInterface`.
    Chromium,
}

/// What first contact did to one process instance.
#[derive(Debug, Clone, Copy)]
struct Contact {
    start_us: u64,
    kind: Kind,
}

/// Windows of Electron apps that have been key once (see `Quirks::wake`).
type Woken = HashSet<(i32, u64, u32)>;

/// One window's first-contact settle: its element count, when that last changed, when the
/// settle began. Pure, so it is tested without accessibility.
#[derive(Debug, Clone, Copy)]
struct Settle {
    count: usize,
    changed: Instant,
    began: Instant,
    /// When the app's launch is over (`LAUNCHING` after its process started).
    launched: Instant,
}

impl Settle {
    fn new(count: usize, now: Instant, launched: Instant) -> Self {
        Self {
            count,
            changed: now,
            began: now,
            launched,
        }
    }

    /// Takes a new count, whether what it counts is usable yet (a page with content), and how
    /// long this window may be waited for.
    fn update(&mut self, count: usize, usable: bool, bound: Duration, now: Instant) -> Structure {
        if count != self.count {
            self.count = count;
            self.changed = now;
        }
        let quiet = now.duration_since(self.changed) >= SETTLE_QUIET && now >= self.launched;
        if usable && quiet {
            Structure::Ready
        } else if now.duration_since(self.began) < bound {
            Structure::Pending
        } else if usable {
            Structure::Ready
        } else {
            // The next look waits a whole bound again: answering "incomplete" at once made a
            // worker spend a model call per look while Chrome was still building its page.
            self.began = now;
            Structure::Incomplete
        }
    }
}

/// A window of one process instance: pid, start time, window id.
type WindowKey = (i32, u64, u32);

#[derive(Default)]
pub struct Quirks {
    contacts: HashMap<i32, Contact>,
    woken: Woken,
    settling: HashMap<WindowKey, Settle>,
    settled: HashSet<WindowKey>,
}

impl Quirks {
    /// Makes first contact with `pid` once per process instance, on its application element:
    /// an Electron or Chromium app is asked for its tree. Cheap after the first call.
    pub fn first_contact(&mut self, pid: i32) -> Kind {
        let start_us = super::process_start_us(pid).unwrap_or(0);
        if let Some(c) = self.contacts.get(&pid)
            && c.start_us == start_us
        {
            return c.kind;
        }
        let kind = kind(pid);
        let app = AxEl::app(pid);
        if kind != Kind::Native {
            let _ = app.set("AXManualAccessibility", CFBoolean::new(true));
        }
        if kind == Kind::Chromium {
            // Chromium reports the call unimplemented, yet builds its page trees from it.
            let _ = app.set("AXEnhancedUserInterface", CFBoolean::new(true));
        }
        self.contacts.insert(pid, Contact { start_us, kind });
        kind
    }

    /// Makes an Electron window key inside its app once, by synthetic activation, then lets it go.
    /// Until a window has been key, Electron serves no page tree for it or leaves its presses
    /// unanswered (measured 2026-10-09: an untouched fixture window gave no web area for 8 s, or
    /// a tree whose presses did nothing; after one activation, both worked from then on). Not
    /// done to the user's own front app, which is active already.
    pub fn wake(&mut self, w: &WindowInfo, user_front: Option<i32>) {
        if self.first_contact(w.pid) != Kind::Electron || user_front == Some(w.pid) {
            return;
        }
        let start_us = self.contacts.get(&w.pid).map_or(0, |c| c.start_us);
        if !self.woken.insert((w.pid, start_us, w.id)) {
            return;
        }
        // Held until the app says the window is key: let go at once, a busy app may never see
        // it key (measured 2026-10-09 under load: one fixture in five then ignored presses).
        let Ok(act) = Activation::begin(w.pid, w.id, true) else {
            return;
        };
        let app = AxEl::app(w.pid);
        let end = Instant::now() + REVEAL_WAIT;
        while Instant::now() < end
            && app.element("AXFocusedWindow").and_then(|el| el.window_id()) != Some(w.id)
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(act);
    }

    /// Whether `w`'s structure is complete: pending while the window's first-contact settle
    /// runs, incomplete when its page never came.
    pub fn structure(&mut self, w: &WindowInfo, window: Option<&AxEl>) -> Structure {
        let kind = self.first_contact(w.pid);
        let Some(window) = window else {
            return Structure::Ready;
        };
        let start_us = self.contacts.get(&w.pid).map_or(0, |c| c.start_us);
        let key = (w.pid, start_us, w.id);
        if self.settled.contains(&key) {
            return Structure::Ready;
        }
        let now = Instant::now();
        let began = self.settling.get(&key).map_or(now, |s| s.began);
        let page = web::content(window);
        let (count, usable, bound) = match page {
            Some(p) => (p.count, p.count > 1 && p.placed, PAGE_BOUND),
            // Electron and Chromium windows hold a page once it is built.
            None if kind != Kind::Native => (0, false, PAGE_BOUND),
            None => {
                // Counted by a full walk, rows out of view included: elements are built as
                // their attributes are first read.
                let count = super::ax::tree(window, origin(w, Some(window)), true).len();
                // An empty view may be a web view whose page comes once it is asked for.
                let waiting = now.duration_since(began) < APPEAR_WAIT && web::empty_host(window);
                (count, !waiting, SETTLE_BOUND)
            }
        };
        let state = match self.settling.get_mut(&key) {
            Some(s) => s.update(count, usable, bound, now),
            None => {
                let age = SystemTime::now()
                    .duration_since(UNIX_EPOCH + Duration::from_micros(start_us))
                    .unwrap_or(LAUNCHING);
                let launching = LAUNCHING.saturating_sub(age);
                self.settling
                    .insert(key, Settle::new(count, now, now + launching));
                Structure::Pending
            }
        };
        if state == Structure::Ready {
            self.settling.remove(&key);
            self.settled.insert(key);
        }
        state
    }

    /// Forgets a window whose element went away, so a new one with its id settles again.
    pub fn forget(&mut self, window_id: u32) {
        self.settled.retain(|k| k.2 != window_id);
        self.settling.retain(|k, _| k.2 != window_id);
    }
}

/// What first contact asks of the app: Electron apps carry Electron's framework in their bundle;
/// Chromium browsers are known by bundle id.
fn kind(pid: i32) -> Kind {
    let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
        return Kind::Native;
    };
    let path = app
        .bundleURL()
        .and_then(|u| u.path())
        .map(|p| p.to_string());
    if path.is_some_and(|p| {
        Path::new(&p)
            .join("Contents/Frameworks/Electron Framework.framework")
            .exists()
    }) {
        return Kind::Electron;
    }
    let bundle = app.bundleIdentifier().map(|b| b.to_string());
    if web::is_chromium(bundle.as_deref()) {
        Kind::Chromium
    } else {
        Kind::Native
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const R: Structure = Structure::Ready;
    const P: Structure = Structure::Pending;

    #[test]
    fn a_tree_counts_as_built_once_its_count_holds() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        let mut s = Settle::new(19, t, t);
        assert_eq!(s.update(19, true, SETTLE_BOUND, ms(50)), P);
        // The lazy stepper arrives.
        assert_eq!(s.update(20, true, SETTLE_BOUND, ms(80)), P);
        assert_eq!(s.update(20, true, SETTLE_BOUND, ms(300)), P);
        assert_eq!(s.update(20, true, SETTLE_BOUND, ms(330)), R);
    }

    #[test]
    fn a_tree_that_keeps_changing_is_taken_after_the_bound() {
        let t = Instant::now();
        let mut s = Settle::new(1, t, t);
        for i in 1..20u64 {
            let at = t + Duration::from_millis(i * 50);
            assert_eq!(s.update(i as usize + 1, true, SETTLE_BOUND, at), P);
        }
        assert_eq!(s.update(99, true, SETTLE_BOUND, t + SETTLE_BOUND), R);
    }

    #[test]
    fn a_just_launched_apps_tree_waits_for_the_launch_to_end() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        // Launched 300 ms before first contact: its launch ends 1.7 s in.
        let mut s = Settle::new(18, t, ms(1700));
        assert_eq!(s.update(18, true, SETTLE_BOUND, ms(400)), P);
        assert_eq!(s.update(18, true, SETTLE_BOUND, ms(1000)), P);
        // The stepper arrives with no read to prompt it.
        assert_eq!(s.update(20, true, SETTLE_BOUND, ms(1150)), P);
        assert_eq!(s.update(20, true, SETTLE_BOUND, ms(1300)), P);
        assert_eq!(s.update(20, true, SETTLE_BOUND, ms(1700)), R);
    }

    #[test]
    fn a_page_is_waited_for_past_two_seconds_then_reported_incomplete() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        let mut s = Settle::new(0, t, t);
        // No page yet: quiet, but not usable.
        assert_eq!(s.update(0, false, PAGE_BOUND, ms(2100)), P);
        assert_eq!(s.update(0, false, PAGE_BOUND, ms(4400)), P);
        assert_eq!(
            s.update(0, false, PAGE_BOUND, t + PAGE_BOUND),
            Structure::Incomplete
        );
        // The next look waits again for the page, and takes it once it holds.
        let next = t + PAGE_BOUND + Duration::from_millis(100);
        assert_eq!(s.update(0, false, PAGE_BOUND, next), P);
        assert_eq!(
            s.update(40, true, PAGE_BOUND, next + Duration::from_millis(100)),
            P
        );
        assert_eq!(
            s.update(
                40,
                true,
                PAGE_BOUND,
                next + Duration::from_millis(100) + SETTLE_QUIET
            ),
            Structure::Ready
        );
    }

    #[test]
    fn a_page_built_at_two_seconds_is_ready_once_it_holds() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        let mut s = Settle::new(1, t, t);
        assert_eq!(s.update(1, false, PAGE_BOUND, ms(2000)), P);
        assert_eq!(s.update(240, true, PAGE_BOUND, ms(2100)), P);
        assert_eq!(s.update(240, true, PAGE_BOUND, ms(2350)), R);
    }
}
