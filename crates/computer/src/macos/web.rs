//! Pages in apps, read through their accessibility web area: what a window's pages hold, and
//! whether an element is in one. First contact with a browser, and the wait for its pages to be
//! built, are `quirks.rs`'s, with every other app's.

use std::time::Duration;

use super::ax::AxEl;
use crate::geom::{Point, Rect};

/// Chromium browsers without a debugging port the session could use, besides those
/// `cdp::CHROMIUM_BUNDLES` names.
const CHROMIUM_SHELLS: &[&str] = &["company.thebrowser.Browser", "company.thebrowser.dia"];
/// The share of its window an empty view covers when it is a page not built yet.
const HOST_SHARE: f64 = 0.25;
/// A walk stops counting here: a page this big is built enough to read.
const COUNT_CAP: usize = 3_000;

pub(super) fn is_chromium(bundle_id: Option<&str>) -> bool {
    crate::cdp::is_chromium(bundle_id)
        || bundle_id.is_some_and(|b| CHROMIUM_SHELLS.iter().any(|c| c.eq_ignore_ascii_case(b)))
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
pub(super) struct WebContent {
    /// Their elements, the areas included (capped).
    pub count: usize,
    /// The element holding every outermost web area overlaps the window.
    pub placed: bool,
}

/// What the window's web areas hold, or None without a web area.
pub(super) fn content(window: &AxEl) -> Option<WebContent> {
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
pub(super) fn empty_host(window: &AxEl) -> bool {
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
