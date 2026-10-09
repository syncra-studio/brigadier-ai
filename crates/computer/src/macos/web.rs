//! Pages in browsers the session didn't launch, read through their accessibility web area.
//! Chromium builds a page's tree only once an assistive app asks for it, and then over about
//! two seconds; WebKit fills a page's tree lazily on first contact. So the first look at a
//! browser's window turns web accessibility on (Chromium) and waits until the page's tree stops
//! growing; later looks read it straight away.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use objc2_core_foundation::CFBoolean;

use super::ax::AxEl;

/// Chromium browsers without a debugging port the session could use, besides those
/// `cdp::CHROMIUM_BUNDLES` names.
const CHROMIUM_SHELLS: &[&str] = &["company.thebrowser.Browser", "company.thebrowser.dia"];
/// How long a first look waits for a page's tree; Chromium alone takes about two seconds.
const READY_WAIT: Duration = Duration::from_secs(5);
/// How often the tree is counted while waiting, and how many equal counts mean it's built.
const STEP: Duration = Duration::from_millis(200);
const STABLE_COUNTS: u32 = 2;
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
        let first = web_content(window);
        if !chromium && first.is_none() {
            // No page in this window: nothing to wait for.
            self.ready.insert(window_id);
            return;
        }
        let started = Instant::now();
        let (mut last, mut same) = (first, 0);
        while started.elapsed() < READY_WAIT {
            std::thread::sleep(STEP);
            let now = web_content(window);
            if now.is_some_and(|n| n > 0) && now == last {
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

/// How many elements the window's web areas hold (capped), or None without a web area.
fn web_content(window: &AxEl) -> Option<usize> {
    let mut stack = vec![(window.clone(), false)];
    let (mut found, mut count, mut walked) = (false, 0usize, 0usize);
    while let Some((el, inside)) = stack.pop() {
        walked += 1;
        if walked > COUNT_CAP * 2 || count >= COUNT_CAP {
            break;
        }
        let web = inside || el.string("AXRole").as_deref() == Some("AXWebArea");
        if web {
            found = true;
            count += 1;
        }
        for c in el.elements("AXChildren") {
            stack.push((c, web));
        }
    }
    found.then_some(count)
}
