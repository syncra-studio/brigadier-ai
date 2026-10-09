//! Pages in browsers the session didn't launch, read through their accessibility web area.
//! Chromium builds a page's tree only once an assistive app asks for it, and then over about
//! two seconds; WebKit fills a page's tree lazily on first contact. So the first look at a
//! browser's window turns web accessibility on (Chromium) and waits until the page's tree stops
//! growing; later looks read it straight away. A WKWebView may also report its scroll area at a
//! stale place on first contact, outside its window: the wait lasts until it is back inside.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use objc2_core_foundation::CFBoolean;

use super::ax::AxEl;
use crate::geom::{Point, Rect};

/// Chromium browsers without a debugging port the session could use, besides those
/// `cdp::CHROMIUM_BUNDLES` names.
const CHROMIUM_SHELLS: &[&str] = &["company.thebrowser.Browser", "company.thebrowser.dia"];
/// How long a first look waits for a page's tree; Chromium alone takes about two seconds.
const READY_WAIT: Duration = Duration::from_secs(5);
/// How often the tree is counted while waiting, and how many equal counts mean it's built.
const STEP: Duration = Duration::from_millis(200);
const STABLE_COUNTS: u32 = 2;
/// How long an empty view is given to show the page it will hold.
const APPEAR_WAIT: Duration = Duration::from_secs(1);
/// The share of its window an empty view covers when it is a page not built yet.
const HOST_SHARE: f64 = 0.25;
/// A walk stops counting here: a page this big is built enough to read.
const COUNT_CAP: usize = 3_000;

fn is_chromium(bundle_id: Option<&str>) -> bool {
    crate::cdp::is_chromium(bundle_id)
        || bundle_id.is_some_and(|b| CHROMIUM_SHELLS.iter().any(|c| c.eq_ignore_ascii_case(b)))
}

/// The browsers turned on, and the windows already waited for.
#[derive(Default)]
pub struct WebAreas {
    enabled: HashMap<i32, bool>,
    ready: HashSet<u32>,
}

impl WebAreas {
    /// Before a window's tree is read: on the first look at a browser's window, makes its
    /// pages readable and waits until they are.
    pub fn prepare(
        &mut self,
        pid: i32,
        window_id: u32,
        window: &AxEl,
        bundle_id: impl FnOnce() -> Option<String>,
    ) {
        if self.ready.contains(&window_id) {
            return;
        }
        let first_contact = !self.enabled.contains_key(&pid);
        let chromium = *self
            .enabled
            .entry(pid)
            .or_insert_with(|| is_chromium(bundle_id().as_deref()));
        if chromium && first_contact {
            // Chromium builds its page trees once an assistive app sets the enhanced interface
            // flag, though the call reports it isn't implemented; shells built on it may want
            // the manual flag instead. Both are set on the app, and stay set for the process.
            let app = AxEl::app(pid);
            let _ = app.set("AXManualAccessibility", CFBoolean::new(true));
            let _ = app.set("AXEnhancedUserInterface", CFBoolean::new(true));
        }
        let mut first = web_content(window);
        if !chromium && first.is_none() {
            // WebKit builds a page's tree once asked for it: until then its view is an empty
            // group. Without one there is no page in this window, and nothing to wait for.
            if empty_host(window) {
                let until = Instant::now() + APPEAR_WAIT;
                while first.is_none() && Instant::now() < until {
                    std::thread::sleep(STEP / 4);
                    first = web_content(window);
                }
            }
            if first.is_none() {
                self.ready.insert(window_id);
                return;
            }
        }
        let started = Instant::now();
        let (mut last, mut same) = (first.map(|w| w.count), 0);
        while started.elapsed() < READY_WAIT {
            std::thread::sleep(STEP);
            let web = web_content(window);
            let now = web.map(|w| w.count);
            if web.is_some_and(|w| w.count > 0 && w.placed) && now == last {
                same += 1;
                if same >= STABLE_COUNTS {
                    break;
                }
            } else {
                same = 0;
            }
            last = now;
        }
        self.ready.insert(window_id);
    }

    /// Forgets a window that closed, so a new one with its id is waited for.
    pub fn forget(&mut self, window_id: u32) {
        self.ready.remove(&window_id);
    }
}

/// How long a page's field may take to report the focus it was given (WebKit: ≈0.5 s at most).
pub const FOCUS_WAIT: Duration = Duration::from_secs(1);
/// How far up an element's ancestors a page is looked for.
const PAGE_DEPTH: usize = 64;
/// How deep in a window an empty view is looked for.
const HOST_DEPTH: usize = 6;

/// Whether the element is inside a web page.
pub fn in_page(el: &AxEl) -> bool {
    let mut at = el.element("AXParent");
    for _ in 0..PAGE_DEPTH {
        let Some(e) = at else { return false };
        match e.string("AXRole").as_deref() {
            Some("AXWebArea") => return true,
            Some("AXWindow" | "AXApplication") | None => return false,
            _ => at = e.element("AXParent"),
        }
    }
    false
}

/// What a window's web areas hold so far.
#[derive(Clone, Copy)]
struct WebContent {
    /// Their elements (capped).
    count: usize,
    /// The element holding every outermost web area overlaps the window.
    placed: bool,
}

/// What the window's web areas hold, or None without a web area.
fn web_content(window: &AxEl) -> Option<WebContent> {
    let mut stack = vec![(window.clone(), false, None)];
    let (mut found, mut placed, mut count, mut walked) = (false, true, 0usize, 0usize);
    while let Some((el, inside, parent)) = stack.pop() {
        walked += 1;
        if walked > COUNT_CAP * 2 || count >= COUNT_CAP {
            break;
        }
        let area = !inside && el.string("AXRole").as_deref() == Some("AXWebArea");
        if area && let Some(holder) = parent {
            placed &= overlaps(&holder, window);
        }
        let web = inside || area;
        if web {
            found = true;
            count += 1;
        }
        for c in el.elements("AXChildren") {
            stack.push((c, web, (!web).then(|| el.clone())));
        }
    }
    found.then_some(WebContent { count, placed })
}

/// Whether the window holds an empty group covering much of it: a view whose content is built
/// on request, as a web view's page is. A button's empty group doesn't count.
fn empty_host(window: &AxEl) -> bool {
    let Some(w) = frame(window) else { return false };
    let mut stack = vec![(window.clone(), 0usize)];
    while let Some((el, depth)) = stack.pop() {
        let kids = el.elements("AXChildren");
        let role = el.string("AXRole");
        if kids.is_empty()
            && role.as_deref() == Some("AXGroup")
            && frame(&el).is_some_and(|f| f.w * f.h >= HOST_SHARE * w.w * w.h)
        {
            return true;
        }
        if depth < HOST_DEPTH && role.as_deref() != Some("AXButton") {
            stack.extend(kids.into_iter().map(|c| (c, depth + 1)));
        }
    }
    false
}

/// An element's frame on screen.
fn frame(e: &AxEl) -> Option<Rect> {
    super::ax::read(e, Point::new(0.0, 0.0)).ok()?.frame
}

/// Whether two elements' frames overlap; true when either can't say.
fn overlaps(a: &AxEl, b: &AxEl) -> bool {
    match (frame(a), frame(b)) {
        (Some(a), Some(b)) => !a.intersect(&b).is_empty(),
        _ => true,
    }
}
