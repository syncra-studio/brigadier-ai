//! The macOS backend (§4.1): accessibility for structure and element actions,
//! ScreenCaptureKit for pixels, pid-posted events for background input.
//!
//! The system APIs are C and Objective-C, so this module is where the crate's unsafe code
//! lives; every block says why it holds.
#![allow(unsafe_code)]

mod ax;
mod capture;
pub(crate) mod input;
pub mod overlay;
pub mod private;
mod quirks;
mod web;

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::{Duration, Instant};

use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};
use objc2_application_services::{AXError, AXObserver, AXUIElement};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFDictionary, CFNumber, CFRetained, CFRunLoop, CFString, CFType,
    kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGEvent, CGEventSource,
    CGEventSourceStateID, CGEventType, CGGetActiveDisplayList, CGWindowListCopyWindowInfo,
    CGWindowListOption, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowName,
    kCGWindowNumber, kCGWindowOwnerPID,
};
use objc2_foundation::{NSBundle, NSString, NSURL};

pub use ax::AxEl;
use private::Private;

use crate::cancel::{CancelToken, InputGuard, Release};
use crate::desktop::{
    AppInfo, Button, Capabilities, Capture, Chord, Desktop, Focus, Mods, PendingCapture, UserFocus,
    WindowInfo,
};
use crate::error::{CuError, CuResult, ErrorCode, err};
use crate::geom::{self, Point, Rect};
use crate::tree::RawNode;

/// Whether this process may use accessibility and capture the screen.
/// How many element ids, and how long, a search for a window on another Space tries.
const REMOTE_SCAN_IDS: u64 = 20_000;
const REMOTE_SCAN_TIME: Duration = Duration::from_millis(1500);

pub fn permissions() -> (bool, bool) {
    // SAFETY: both are plain queries.
    unsafe {
        (
            objc2_application_services::AXIsProcessTrusted(),
            objc2_core_graphics::CGPreflightScreenCaptureAccess(),
        )
    }
}

/// Asks the system for a permission: it shows its own prompt the first time and lists this
/// process in System Settings, where the user grants it. Returns at once; the grant comes
/// later, if at all.
pub fn request_permission(grant: crate::wire::Grant) {
    match grant {
        crate::wire::Grant::Accessibility => {
            // SAFETY: a static CFString key the framework exports.
            let key = unsafe { objc2_application_services::kAXTrustedCheckOptionPrompt };
            let options =
                CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[CFBoolean::new(true)]);
            // SAFETY: the options dictionary maps the documented key to a CFBoolean.
            unsafe {
                objc2_application_services::AXIsProcessTrustedWithOptions(Some(
                    options.as_opaque(),
                ));
            }
        }
        crate::wire::Grant::ScreenRecording => {
            objc2_core_graphics::CGRequestScreenCaptureAccess();
        }
    }
}

/// When the process `pid` started, in microseconds since the Unix epoch; `None` when there is
/// no such process. With the pid it tells a process apart from a later one that reuses its pid.
pub fn process_start_us(pid: i32) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    // SAFETY: `proc_bsdinfo` is plain data, valid when zeroed.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is a `proc_bsdinfo` of exactly `size` bytes, which is what
    // PROC_PIDTBSDINFO writes.
    let n =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    (n == size).then(|| info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

/// How long a live process may be missing from the running apps after its launch.
const APP_KNOWN: Duration = Duration::from_secs(1);

/// The accessibility notifications that mean an app is still changing.
const NOTIFICATIONS: &[&str] = &[
    "AXValueChanged",
    "AXFocusedUIElementChanged",
    "AXFocusedWindowChanged",
    "AXMainWindowChanged",
    "AXWindowCreated",
    "AXWindowMoved",
    "AXWindowResized",
    "AXUIElementDestroyed",
    "AXCreated",
    "AXTitleChanged",
    "AXMenuOpened",
    "AXMenuClosed",
    "AXSelectedChildrenChanged",
    "AXSelectedTextChanged",
    "AXSelectedRowsChanged",
    "AXRowCountChanged",
    "AXLayoutChanged",
    "AXSheetCreated",
];

struct Watch {
    _observer: CFRetained<AXObserver>,
    last: Box<Cell<Option<Instant>>>,
}

unsafe extern "C-unwind" fn on_notification(
    _o: NonNull<AXObserver>,
    _e: NonNull<AXUIElement>,
    _n: NonNull<CFString>,
    refcon: *mut c_void,
) {
    // SAFETY: `refcon` is the boxed cell of a live `Watch` (removed only with its observer).
    let cell = unsafe { &*(refcon as *const Cell<Option<Instant>>) };
    cell.set(Some(Instant::now()));
}

pub struct MacDesktop {
    shareable: capture::Shareable,
    watches: HashMap<i32, Watch>,
    /// Accessibility windows by window-server id.
    ax_windows: HashMap<u32, AxEl>,
    /// The synthetic activation the batch's actions share, with its app (`end_batch` ends it).
    held: Option<(i32, input::Activation)>,
    quirks: quirks::Quirks,
}

impl MacDesktop {
    pub fn new() -> CuResult<Self> {
        let (ax, _) = permissions();
        if !ax {
            return err(
                ErrorCode::PermissionMissing,
                "the Accessibility permission is missing",
            );
        }
        Ok(Self {
            shareable: capture::Shareable::default(),
            watches: HashMap::new(),
            ax_windows: HashMap::new(),
            held: None,
            quirks: quirks::Quirks::default(),
        })
    }

    /// Brings `pid` to the front (with `window` key when not 0) and waits until it is: through
    /// the window server, else accessibility. A background process's activation calls are
    /// declined (measured 2026-10-09: `AXFrontmost` reported success and nothing moved).
    fn front(&mut self, pid: i32, window: u32) -> CuResult<()> {
        if !Private::get().set_front(pid, window) {
            AxEl::app(pid).set("AXFrontmost", CFBoolean::new(true))?;
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if front_pid() == Some(pid) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        err(
            ErrorCode::Failed,
            format!("pid {pid} didn't come to the front"),
        )
    }

    fn ax_window(&mut self, w: &WindowInfo) -> CuResult<AxEl> {
        if let Some(el) = self.ax_windows.get(&w.id) {
            return Ok(el.clone());
        }
        if self.quirks.first_contact(w.pid) == quirks::Kind::Electron {
            // Waking it makes it key inside its app, which would end an activation the batch
            // holds on another of its windows.
            if self.held.as_ref().is_some_and(|(pid, _)| *pid == w.pid) {
                self.release_activation();
            }
            self.quirks.wake(w, front_pid());
        }
        let app = AxEl::app(w.pid);
        // The window list holds only the current Space's windows. The app's main and focused
        // windows are given wherever they are (measured 2026-10-09 with the user on a full-screen
        // Space); any other window on another Space is found by remote token.
        let listed = app.elements("AXWindows").into_iter();
        let named = ["AXMainWindow", "AXFocusedWindow"]
            .into_iter()
            .filter_map(|a| app.element(a));
        for el in listed.chain(named) {
            if let Some(id) = el.window_id() {
                self.ax_windows.insert(id, el);
            }
        }
        if !self.ax_windows.contains_key(&w.id) && !w.on_screen {
            // Revealing makes the window key inside its app, which would end an activation
            // the batch holds on another of its windows.
            if self.held.as_ref().is_some_and(|(pid, _)| *pid == w.pid) {
                self.release_activation();
            }
            let front = front_pid();
            if let Some(el) = quirks::reveal(w, front) {
                self.ax_windows.insert(w.id, el);
            }
        }
        if !self.ax_windows.contains_key(&w.id) {
            self.remote_windows(w);
        }
        self.ax_windows.get(&w.id).cloned().ok_or_else(|| {
            let why = if w.on_screen || w.minimized {
                ""
            } else {
                ": it is off screen and its app didn't make it key, so it is ordered out (shown \
                 on no Space) or its app is your front app"
            };
            CuError::new(
                ErrorCode::NoSuchTarget,
                format!("window w{} has no accessibility element{why}", w.id),
            )
        })
    }

    /// A window on another Space that is neither the app's main nor its focused window, found
    /// among the app's elements by remote token: ids from 0 up until it turns up (measured
    /// 2026-10-09: windows sat at ids 42–43 of fresh apps; 2,000 ids took 28–73 ms). Only an
    /// element some accessibility client already reached has an id.
    fn remote_windows(&mut self, w: &WindowInfo) {
        let deadline = Instant::now() + REMOTE_SCAN_TIME;
        for id in 0..REMOTE_SCAN_IDS {
            if id % 64 == 0 && Instant::now() > deadline {
                return;
            }
            let Some(el) = AxEl::remote(w.pid, id) else {
                continue;
            };
            if el.string("AXRole").as_deref() != Some("AXWindow") {
                continue;
            }
            if let Some(found) = el.window_id() {
                self.ax_windows.insert(found, el);
                if found == w.id {
                    return;
                }
            }
        }
    }

    fn window_list(option: CGWindowListOption, relative_to: u32) -> Vec<WindowInfo> {
        let Some(list) = CGWindowListCopyWindowInfo(option, relative_to) else {
            return Vec::new();
        };
        // SAFETY: the window list is an array of dictionaries with string keys.
        let list: CFRetained<CFArray<CFDictionary<CFString, CFType>>> =
            unsafe { CFRetained::cast_unchecked(list) };
        let num = |d: &CFDictionary<CFString, CFType>, k: &CFString| -> Option<f64> {
            d.get(k)
                .and_then(|v| v.downcast::<CFNumber>().ok())
                .and_then(|n| n.as_f64())
        };
        let displays = display_bounds();
        let mut out = Vec::new();
        for d in list.iter() {
            // SAFETY: reading the system's constant keys.
            let (k_layer, k_num, k_pid, k_name, k_bounds, k_on) = unsafe {
                (
                    kCGWindowLayer,
                    kCGWindowNumber,
                    kCGWindowOwnerPID,
                    kCGWindowName,
                    kCGWindowBounds,
                    kCGWindowIsOnscreen,
                )
            };
            if num(&d, k_layer) != Some(0.0) {
                continue;
            }
            let (Some(id), Some(pid)) = (num(&d, k_num), num(&d, k_pid)) else {
                continue;
            };
            let title = d
                .get(k_name)
                .and_then(|v| v.downcast::<CFString>().ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let frame = d
                .get(k_bounds)
                .and_then(|v| v.downcast::<CFDictionary>().ok())
                .map(|b| {
                    // SAFETY: the bounds dictionary has string keys and number values.
                    let b: CFRetained<CFDictionary<CFString, CFType>> =
                        unsafe { CFRetained::cast_unchecked(b) };
                    let g = |k: &str| num(&b, &CFString::from_str(k)).unwrap_or(0.0);
                    Rect::new(g("X"), g("Y"), g("Width"), g("Height"))
                })
                .unwrap_or_default();
            let on_screen = d
                .get(k_on)
                .and_then(|v| v.downcast::<CFBoolean>().ok())
                .is_some_and(|b| b.as_bool());
            if frame.w < 2.0 || frame.h < 2.0 || is_menu_bar_strip(&title, frame, &displays) {
                continue;
            }
            out.push(WindowInfo {
                id: id as u32,
                pid: pid as i32,
                title,
                frame,
                on_screen,
                minimized: false,
                hidden: false,
            });
        }
        out
    }

    fn app_info(app: &NSRunningApplication, front: Option<i32>) -> AppInfo {
        let pid = app.processIdentifier();
        AppInfo {
            pid,
            name: app
                .localizedName()
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("pid {pid}")),
            bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
            bundle_path: app
                .bundleURL()
                .and_then(|u| u.path())
                .map(|p| p.to_string()),
            frontmost: front == Some(pid),
            windows: Vec::new(),
        }
    }
}

impl MacDesktop {
    /// Makes `w`'s app believe it is active with `w` key until the batch ends. Activating it
    /// once a batch, not once an action, keeps its window from flashing its active look with
    /// every click.
    fn hold_activation(&mut self, w: &WindowInfo) -> CuResult<()> {
        if self.held.as_ref().is_some_and(|(_, a)| a.window() == w.id) {
            return Ok(());
        }
        self.release_activation();
        self.held = Some((w.pid, input::Activation::begin(w.pid, w.id, true)?));
        Ok(())
    }

    fn release_activation(&mut self) {
        // Dropped otherwise, which sends the defocus.
        if let Some((pid, act)) = self.held.take()
            && self.user_focus().frontmost_pid == pid
        {
            act.forget();
        }
    }
}

impl Desktop for MacDesktop {
    type Element = AxEl;

    fn structure(&mut self, w: &WindowInfo) -> crate::desktop::Structure {
        let el = self.ax_window(w).ok();
        self.quirks.structure(w, el.as_ref())
    }

    fn releaser(&self) -> Arc<dyn Release + Send + Sync> {
        Arc::new(input::Releaser)
    }

    fn capabilities(&self) -> Capabilities {
        let p = Private::get();
        Capabilities {
            structure: true,
            capture: true,
            element_actions: true,
            background_keys: true,
            background_pointer: p.has_pointer(),
            synthetic_activation: p.has_activation(),
        }
    }

    fn apps(&mut self) -> CuResult<Vec<AppInfo>> {
        // The workspace's list of running apps only updates while this thread's run loop
        // runs: without a turn, a long-lived helper never sees an app started after it read
        // the list once.
        self.pump(Duration::ZERO);
        let ws = NSWorkspace::sharedWorkspace();
        let front = ws.frontmostApplication().map(|a| a.processIdentifier());
        let windows = Self::window_list(
            CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements,
            0,
        );
        let mut out = Vec::new();
        for app in ws.runningApplications().iter() {
            let mut info = Self::app_info(&app, front);
            // A listing hint only, with no accessibility call per app: an action reads it again.
            let hidden = app.isHidden();
            info.windows = windows
                .iter()
                .filter(|w| w.pid == info.pid)
                .map(|w| WindowInfo {
                    hidden: hidden && !w.on_screen,
                    ..w.clone()
                })
                .collect();
            if info.windows.is_empty()
                && app.activationPolicy() != NSApplicationActivationPolicy::Regular
            {
                continue;
            }
            out.push(info);
        }
        Ok(out)
    }

    fn windows(&mut self, pid: i32) -> CuResult<Vec<WindowInfo>> {
        let all = Self::window_list(
            CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements,
            0,
        );
        let mut mine: Vec<WindowInfo> = all.into_iter().filter(|w| w.pid == pid).collect();
        if mine.iter().any(|w| !w.on_screen) && AxEl::app(pid).bool("AXHidden") == Some(true) {
            for w in &mut mine {
                w.hidden = true;
            }
        }
        Ok(mine)
    }

    fn window(&mut self, id: u32) -> CuResult<WindowInfo> {
        // The one-window query skips windows that aren't on screen (minimised, hidden).
        let mut w = Self::window_list(CGWindowListOption::OptionIncludingWindow, id)
            .into_iter()
            .find(|w| w.id == id)
            .or_else(|| {
                Self::window_list(CGWindowListOption::OptionAll, 0)
                    .into_iter()
                    .find(|w| w.id == id)
            })
            .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, format!("no window w{id}")))?;
        if !w.on_screen {
            w.hidden = AxEl::app(w.pid).bool("AXHidden") == Some(true);
            if let Ok(el) = self.ax_window(&w) {
                w.minimized = el.bool("AXMinimized").unwrap_or(false);
            }
        }
        Ok(w)
    }

    fn app(&mut self, pid: i32) -> CuResult<AppInfo> {
        let front = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier());
        // With AppKit running on the main thread, a process that just launched can be missing
        // from the running apps for a moment; one that is alive is waited for, briefly.
        let end = Instant::now() + APP_KNOWN;
        let mut found = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        while found.is_none() && Instant::now() < end && unsafe { libc::kill(pid, 0) } == 0 {
            std::thread::sleep(Duration::from_millis(20));
            found = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        }
        found
            .map(|a| Self::app_info(&a, front))
            .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, format!("no app with pid {pid}")))
    }

    fn tree(&mut self, w: &WindowInfo, all: bool) -> CuResult<Vec<RawNode<AxEl>>> {
        let el = self.ax_window(w)?;
        let nodes = ax::tree(&el, quirks::origin(w, Some(&el)), all);
        if nodes.len() <= 1 && el.attr("AXRole").is_err() {
            self.ax_windows.remove(&w.id);
            self.quirks.forget(w.id);
            return err(
                ErrorCode::StaleRef,
                "the window's accessibility element went away; observe again",
            );
        }
        Ok(nodes)
    }

    fn read(&mut self, w: &WindowInfo, el: &AxEl) -> CuResult<RawNode<AxEl>> {
        let window = self.ax_window(w).ok();
        ax::read(el, quirks::origin(w, window.as_ref()))
    }

    fn backing_scale(&mut self, w: &WindowInfo) -> f64 {
        geom::scale_at(&displays(), w.frame.center())
    }

    fn capture(
        &mut self,
        w: &WindowInfo,
        crop: Rect,
        pixels_per_point: f64,
        max_side: u32,
    ) -> CuResult<Capture> {
        let (image, transform) = if w.on_screen {
            self.shareable
                .capture(w.id, w.frame, crop, pixels_per_point, max_side)?
        } else {
            capture::capture_offscreen(w.id, w.frame, crop, pixels_per_point, max_side)?
        };
        Ok(Capture { image, transform })
    }

    fn begin_capture(
        &mut self,
        w: &WindowInfo,
        crop: Rect,
        pixels_per_point: f64,
        max_side: u32,
    ) -> CuResult<PendingCapture> {
        if !w.on_screen {
            let done = self.capture(w, crop, pixels_per_point, max_side);
            return Ok(PendingCapture::new(move || done));
        }
        let wait = self
            .shareable
            .begin(w.id, w.frame, crop, pixels_per_point, max_side)?;
        Ok(PendingCapture::new(move || {
            let (image, transform) = wait()?;
            Ok(Capture { image, transform })
        }))
    }

    fn perform(&mut self, el: &AxEl, action: &str) -> CuResult<()> {
        let name = ax::ax_action(action);
        if !el.action_names().contains(&name) {
            return err(
                ErrorCode::NoSuchAction,
                format!("the element has no {action} action"),
            );
        }
        el.perform_bounded(&name, PERFORM_REPLY_WAIT)
    }

    fn set_value(&mut self, el: &AxEl, text: &str) -> CuResult<()> {
        let role = el.string("AXRole").unwrap_or_default();
        if role == "AXPopUpButton" {
            let windows: Vec<AxEl> = self.ax_windows.values().cloned().collect();
            return pick_popup(el, text, &windows);
        }
        let numeric = el
            .attr("AXValue")
            .ok()
            .is_some_and(|v| v.downcast::<CFNumber>().is_ok());
        if numeric {
            let n: f64 = text.trim().parse().map_err(|_| {
                CuError::new(ErrorCode::BadRequest, format!("{text:?} is not a number"))
            })?;
            let v = CFNumber::new_f64(n);
            return el.set("AXValue", &v);
        }
        // A web view gives a value to the field focused within its page, whichever field it was
        // sent to: so a page's field is focused first (its app's focus, not the user's).
        if web::in_page(el) && el.bool("AXFocused") != Some(true) && el.settable("AXFocused") {
            el.set("AXFocused", CFBoolean::new(true))?;
            let until = Instant::now() + web::FOCUS_WAIT;
            while el.bool("AXFocused") != Some(true) {
                if Instant::now() > until {
                    return err(
                        ErrorCode::NotSettable,
                        "the field didn't take focus in its page",
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        el.set("AXValue", &CFString::from_str(text))
    }

    fn insert_text(&mut self, el: &AxEl, text: &str) -> CuResult<()> {
        if !el.settable("AXSelectedText") {
            return err(ErrorCode::NotSettable, "no selection to insert at");
        }
        el.set("AXSelectedText", &CFString::from_str(text))
    }

    fn set_focus(&mut self, el: &AxEl) -> CuResult<()> {
        el.set("AXFocused", CFBoolean::new(true))
    }

    fn select(&mut self, el: &AxEl, start: usize, length: usize) -> CuResult<()> {
        if !el.settable("AXSelectedTextRange") {
            return err(ErrorCode::NotSettable, "that element has no text selection");
        }
        let (start, length) = match el.string("AXValue") {
            Some(text) => to_utf16(&text, start, length),
            None => (start, length),
        };
        el.set_range("AXSelectedTextRange", start, length)
    }

    fn selection(&mut self, el: &AxEl) -> Option<(usize, usize)> {
        let (start, length) = el.range("AXSelectedTextRange")?;
        Some(match el.string("AXValue") {
            Some(text) => to_chars(&text, start, length),
            None => (start, length),
        })
    }

    fn menu(&mut self, pid: i32, path: &[String]) -> CuResult<()> {
        let item = menu_item(pid, path)?;
        if item.bool("AXEnabled") == Some(false) {
            let name = path.last().map(String::as_str).unwrap_or("");
            return err(ErrorCode::Failed, format!("menu item {name:?} is disabled"));
        }
        item.perform("AXPress")
    }

    fn focus(&mut self, pid: i32) -> CuResult<Focus<AxEl>> {
        let app = AxEl::app(pid);
        let el = app.element("AXFocusedUIElement");
        let window = el
            .as_ref()
            .and_then(|e| e.element("AXWindow"))
            .or_else(|| app.element("AXFocusedWindow"))
            .and_then(|w| w.window_id());
        let role = el.as_ref().and_then(|e| e.string("AXRole"));
        let secure =
            el.as_ref().and_then(|e| e.string("AXSubrole")).as_deref() == Some("AXSecureTextField");
        let selected_text = el
            .as_ref()
            .filter(|_| !secure)
            .and_then(|e| e.string("AXSelectedText"))
            .filter(|t| !t.is_empty());
        Ok(Focus {
            element: el,
            window,
            secure,
            role,
            selected_text,
        })
    }

    fn click(
        &mut self,
        w: &WindowInfo,
        at: Point,
        button: Button,
        count: u8,
        mods: Mods,
        activate: bool,
        guard: &mut InputGuard<'_>,
    ) -> CuResult<()> {
        if activate {
            self.hold_activation(w)?;
        }
        input::click(w, at, button, count, mods, false, guard)
    }

    fn scroll(&mut self, w: &WindowInfo, at: Point, dx: i32, dy: i32) -> CuResult<()> {
        input::scroll(w, at, dx, dy)
    }

    fn drag(
        &mut self,
        w: &WindowInfo,
        from: Point,
        to: Point,
        activate: bool,
        guard: &mut InputGuard<'_>,
        cancel: &CancelToken,
    ) -> CuResult<()> {
        if activate {
            self.hold_activation(w)?;
        }
        input::drag(w, from, to, false, guard, cancel)
    }

    fn key(&mut self, pid: i32, chord: &Chord, guard: &mut InputGuard<'_>) -> CuResult<()> {
        input::key(pid, chord, guard)
    }

    fn type_text(&mut self, pid: i32, text: &str, cancel: &CancelToken) -> CuResult<()> {
        input::type_text(pid, text, cancel)
    }

    fn menu_for(
        &mut self,
        w: &WindowInfo,
        path: &[String],
        guard: &mut InputGuard<'_>,
    ) -> CuResult<crate::action::Rung> {
        match self.menu(w.pid, path) {
            Ok(()) => Ok(crate::action::Rung::Element),
            // An app checks its menu items only as a menu opens or a shortcut arrives, and a
            // background app checks them against no key window. Its shortcut, sent while the
            // app believes it is active with `w` key, has the item checked then, as for a person.
            Err(e) if e.code == ErrorCode::Failed && e.detail.ends_with("is disabled") => {
                let Some(chord) = menu_item(w.pid, path).ok().as_ref().and_then(shortcut_of) else {
                    return Err(e);
                };
                self.hold_activation(w)?;
                input::key(w.pid, &chord, guard)?;
                Ok(crate::action::Rung::BackgroundActivated)
            }
            Err(e) => Err(e),
        }
    }

    fn shortcut(
        &mut self,
        w: &WindowInfo,
        chord: &Chord,
        guard: &mut InputGuard<'_>,
    ) -> CuResult<crate::action::Rung> {
        self.hold_activation(w)?;
        input::key(w.pid, chord, guard)?;
        Ok(crate::action::Rung::BackgroundActivated)
    }

    fn watch(&mut self, pid: i32) {
        if self.watches.contains_key(&pid) {
            return;
        }
        let mut raw: *mut AXObserver = std::ptr::null_mut();
        // SAFETY: `raw` is a valid out pointer; the callback matches the expected signature.
        let e = unsafe { AXObserver::create(pid, Some(on_notification), NonNull::from(&mut raw)) };
        let Some(obs) = (e == AXError::Success)
            .then_some(raw)
            .and_then(NonNull::new)
        else {
            return;
        };
        // SAFETY: a created observer is +1 retained.
        let obs = unsafe { CFRetained::from_raw(obs) };
        let last = Box::new(Cell::new(None));
        let app = AxEl::app(pid);
        for n in NOTIFICATIONS {
            let name = CFString::from_static_str(n);
            // SAFETY: the refcon points at the boxed cell, which lives as long as the observer.
            unsafe {
                obs.add_notification(
                    &app.0,
                    &name,
                    (&*last as *const Cell<Option<Instant>>).cast_mut().cast(),
                )
            };
        }
        // SAFETY: the run loop source belongs to the live observer.
        let source = unsafe { obs.run_loop_source() };
        if let Some(rl) = CFRunLoop::current() {
            // SAFETY: reading the system's constant mode name.
            rl.add_source(Some(&source), unsafe { kCFRunLoopDefaultMode });
        }
        self.watches.insert(
            pid,
            Watch {
                _observer: obs,
                last,
            },
        );
    }

    fn pump(&mut self, d: Duration) {
        // SAFETY: running this thread's run loop for a bounded time.
        unsafe { CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, d.as_secs_f64(), false) };
    }

    fn last_notification(&self, pid: i32) -> Option<Instant> {
        self.watches.get(&pid).and_then(|w| w.last.get())
    }

    fn end_batch(&mut self) {
        self.release_activation();
    }

    fn user_focus(&mut self) -> UserFocus {
        let front = front_pid().unwrap_or(0);
        let focused = (front != 0)
            .then(|| AxEl::app(front).element("AXFocusedWindow"))
            .flatten();
        let frontmost_window = focused.as_ref().and_then(|w| w.string("AXTitle"));
        let frontmost_window_id = focused.as_ref().and_then(AxEl::window_id);
        let cursor = CGEvent::new(None)
            .map(|e| CGEvent::location(Some(&e)))
            .map(|p| Point::new(p.x, p.y))
            .unwrap_or(Point::new(0.0, 0.0));
        let server_front = Private::get()
            .server_front()
            .map(|p| (u64::from(p.high) << 32) | u64::from(p.low));
        UserFocus {
            frontmost_pid: front,
            frontmost_window,
            frontmost_window_id,
            cursor,
            server_front,
        }
    }

    fn idle_source(&self) -> Arc<dyn Fn() -> f64 + Send + Sync> {
        // The combined session state: events this crate posts to one app don't reset it,
        // where the HID system's state (and IOHIDSystem's HIDIdleTime) do, measured 2026-10-09:
        // three background clicks took both to 0.1 s while this stayed at hours. `!0` is the
        // system's "any input" event type.
        Arc::new(|| {
            CGEventSource::seconds_since_last_event_type(
                CGEventSourceStateID::CombinedSessionState,
                CGEventType(!0),
            )
        })
    }

    fn raise(&mut self, w: &WindowInfo) -> CuResult<()> {
        let el = self.ax_window(w)?;
        let was_minimized = el.bool("AXMinimized") == Some(true);
        if was_minimized {
            el.set("AXMinimized", CFBoolean::new(false))?;
        }
        el.perform("AXRaise")?;
        let _ = el.set("AXMain", CFBoolean::new(true));
        self.front(w.pid, w.id)?;
        if was_minimized {
            // Events sent while the window flies out of the Dock are lost: wait until it is on
            // screen and its frame holds still.
            let deadline = Instant::now() + Duration::from_millis(1500);
            let mut last: Option<Rect> = None;
            while Instant::now() < deadline {
                let now = self.window(w.id)?;
                if now.on_screen && !now.minimized && last == Some(now.frame) {
                    return Ok(());
                }
                last = Some(now.frame);
                std::thread::sleep(Duration::from_millis(40));
            }
        }
        Ok(())
    }

    fn activate(&mut self, pid: i32) -> CuResult<()> {
        self.front(pid, 0)
    }

    fn minimize(&mut self, w: &WindowInfo) -> CuResult<()> {
        self.ax_window(w)?.set("AXMinimized", CFBoolean::new(true))
    }

    fn hide(&mut self, pid: i32) -> CuResult<()> {
        AxEl::app(pid).set("AXHidden", CFBoolean::new(true))
    }

    fn document(&mut self, w: &WindowInfo) -> Option<String> {
        self.ax_window(w).ok()?.string("AXDocument")
    }

    fn close(&mut self, w: &WindowInfo) -> CuResult<()> {
        let button = self.ax_window(w)?.element("AXCloseButton").ok_or_else(|| {
            CuError::new(
                ErrorCode::UnsupportedCapability,
                format!("window w{} has no close button", w.id),
            )
        })?;
        button.perform("AXPress")?;
        self.ax_windows.remove(&w.id);
        Ok(())
    }

    fn resolve(&mut self, app: Option<&str>, target: Option<&str>) -> Option<AppInfo> {
        resolve(app, target)
    }

    fn open(&mut self, app: Option<&str>, target: Option<&str>) -> CuResult<()> {
        // open(1): -g keeps the app out of the foreground; -b names a bundle id, -a a name or a
        // path.
        let mut cmd = std::process::Command::new("/usr/bin/open");
        cmd.arg("-g");
        if let Some(app) = app {
            cmd.arg(if is_bundle_id(app) { "-b" } else { "-a" })
                .arg(app);
        }
        if let Some(t) = target {
            cmd.arg(t);
        }
        let out = cmd
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| CuError::new(ErrorCode::Failed, format!("open: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let why = String::from_utf8_lossy(&out.stderr);
        err(
            ErrorCode::NoSuchTarget,
            format!("couldn't open it: {}", why.trim()),
        )
    }

    fn open_new(&mut self, path: &str, args: &[String]) -> CuResult<()> {
        // open(1): -n a new instance, -g not in front, --args the rest to the app's main().
        let out = std::process::Command::new("/usr/bin/open")
            .args(["-n", "-g", "-a", path, "--args"])
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| CuError::new(ErrorCode::Failed, format!("open: {e}")))?;
        if out.status.success() {
            return Ok(());
        }
        let why = String::from_utf8_lossy(&out.stderr);
        err(
            ErrorCode::NoSuchTarget,
            format!("couldn't open it: {}", why.trim()),
        )
    }
}

/// A range in characters as accessibility counts it, in UTF-16 units: an emoji is two.
fn to_utf16(text: &str, start: usize, length: usize) -> (usize, usize) {
    let units = |chars: usize| text.chars().take(chars).map(char::len_utf16).sum::<usize>();
    let (from, to) = (units(start), units(start.saturating_add(length)));
    (from, to - from)
}

/// An accessibility range, in UTF-16 units, in characters; a unit inside a character counts
/// that whole character.
fn to_chars(text: &str, start: usize, length: usize) -> (usize, usize) {
    let chars = |units: usize| {
        let mut seen = 0;
        text.chars()
            .take_while(|c| {
                let inside = seen < units;
                seen += c.len_utf16();
                inside
            })
            .count()
    };
    let (from, to) = (chars(start), chars(start.saturating_add(length)));
    (from, to - from)
}

/// The app `open` would run for `app` and `target`, read from LaunchServices.
fn resolve(app: Option<&str>, target: Option<&str>) -> Option<AppInfo> {
    let ws = NSWorkspace::sharedWorkspace();
    let url = match (app, target) {
        (Some(app), _) if is_bundle_id(app) => {
            ws.URLForApplicationWithBundleIdentifier(&NSString::from_str(app))?
        }
        (Some(app), _) if app.contains('/') => NSURL::fileURLWithPath(&NSString::from_str(app)),
        // The lookup by name `open -a` makes; deprecated for bundle ids, which a request
        // naming the app doesn't have.
        #[allow(deprecated)]
        (Some(app), _) => {
            let path = ws.fullPathForApplication(&NSString::from_str(app))?;
            NSURL::fileURLWithPath(&path)
        }
        (None, Some(t)) if t.trim_end_matches('/').ends_with(".app") => {
            NSURL::fileURLWithPath(&NSString::from_str(t))
        }
        (None, Some(t)) => {
            let target = if t.contains("://") {
                NSURL::URLWithString(&NSString::from_str(t))?
            } else {
                NSURL::fileURLWithPath(&NSString::from_str(t))
            };
            ws.URLForApplicationToOpenURL(&target)?
        }
        (None, None) => return None,
    };
    let path = url.path()?.to_string();
    let bundle_id = NSBundle::bundleWithURL(&url)
        .and_then(|b| b.bundleIdentifier())
        .map(|b| b.to_string());
    let name = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim_end_matches(".app")
        .to_owned();
    Some(AppInfo {
        pid: -1,
        name,
        bundle_id,
        bundle_path: Some(path),
        frontmost: false,
        windows: Vec::new(),
    })
}

/// Whether `open` names an app by bundle id (`com.apple.TextEdit`) rather than by name or path.
fn is_bundle_id(app: &str) -> bool {
    !app.contains('/')
        && !app.contains(' ')
        && !app.ends_with(".app")
        && app.split('.').count() >= 3
}

/// The frontmost app's pid: the window server's, else AppKit's.
fn front_pid() -> Option<i32> {
    Private::get().front_pid().or_else(|| {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier())
    })
}

/// The active displays' bounds, in global points, and pixels per point, the main display first.
fn displays() -> Vec<(Rect, f64)> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: both pointers are valid for the sizes given.
    unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) };
    ids[..(n as usize).min(ids.len())]
        .iter()
        .map(|&id| {
            let b = CGDisplayBounds(id);
            let scale = CGDisplayCopyDisplayMode(id)
                .map(|m| {
                    let (px, pt) = (
                        CGDisplayMode::pixel_width(Some(&m)),
                        CGDisplayMode::width(Some(&m)),
                    );
                    if pt == 0 { 2.0 } else { px as f64 / pt as f64 }
                })
                .unwrap_or(2.0);
            (
                Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height),
                scale,
            )
        })
        .collect()
}

fn display_bounds() -> Vec<Rect> {
    displays().into_iter().map(|(b, _)| b).collect()
}

/// An app's own strip of the menu bar: AppKit creates it the first time the app is active
/// (synthetic activation included). It is untitled, as wide as a display and sits on its top
/// edge; it is not a window anyone opened.
fn is_menu_bar_strip(title: &str, frame: Rect, displays: &[Rect]) -> bool {
    title.is_empty()
        && frame.h <= 44.0
        && displays.iter().any(|d| {
            (frame.x - d.x).abs() < 0.5
                && (frame.y - d.y).abs() < 0.5
                && (frame.w - d.w).abs() < 0.5
        })
}

/// The menu-bar item at `path`.
fn menu_item(pid: i32, path: &[String]) -> CuResult<AxEl> {
    let Some(bar) = AxEl::app(pid).element("AXMenuBar") else {
        return err(ErrorCode::NoSuchTarget, "the app has no menu bar");
    };
    let mut level = bar.elements("AXChildren");
    for (i, name) in path.iter().enumerate() {
        let want = norm(name);
        let Some(item) = level
            .iter()
            .find(|e| e.string("AXTitle").is_some_and(|t| norm(&t) == want))
            .cloned()
        else {
            // What is there, so the next try needs no other way to read the menus.
            let titles: Vec<String> = level
                .iter()
                .filter_map(|e| e.string("AXTitle"))
                .filter(|t| !t.trim().is_empty())
                .collect();
            let place = match i {
                0 => "the menu bar".to_owned(),
                _ => path[..i].join(" › "),
            };
            return err(
                ErrorCode::NoSuchTarget,
                format!(
                    "no menu item {name:?} in {place}; it has: {}",
                    titles.join(", ")
                ),
            );
        };
        if i + 1 == path.len() {
            return Ok(item);
        }
        // A bar item or submenu item holds one menu whose children are the items.
        level = item
            .elements("AXChildren")
            .into_iter()
            .flat_map(|m| m.elements("AXChildren"))
            .collect();
    }
    err(ErrorCode::BadRequest, "an empty menu path")
}

/// A menu item's keyboard shortcut. `AXMenuItemCmdModifiers` bits: 1 shift, 2 option,
/// 4 control, 8 no command key (`HIToolbox/Menus.h`, `kMenu*Modifier`).
fn shortcut_of(item: &AxEl) -> Option<Chord> {
    let key = item.string("AXMenuItemCmdChar")?.to_lowercase();
    if key.trim().is_empty() {
        return None;
    }
    let bits = item
        .attr("AXMenuItemCmdModifiers")
        .ok()
        .and_then(|v| v.downcast::<CFNumber>().ok())
        .and_then(|n| n.as_i64())
        .unwrap_or(0);
    Some(Chord {
        mods: Mods {
            cmd: bits & 8 == 0,
            shift: bits & 1 != 0,
            alt: bits & 2 != 0,
            ctrl: bits & 4 != 0,
        },
        key,
    })
}

fn norm(s: &str) -> String {
    s.trim().replace("...", "…").to_lowercase()
}

/// How long an element action waits for the app's reply. An app answers only when its
/// handler returns, and a button's handler includes its highlight (≈100 ms) after the action
/// has already run; the engine's effect check reads the outcome instead of waiting for that.
const PERFORM_REPLY_WAIT: Duration = Duration::from_millis(3);

/// Picks a pop-up button's item by title through its menu. The menu's items only exist while
/// it is open, so it is opened first and the item pressed at once; a background app's menu
/// doesn't take the user's focus. AppKit blinks the chosen item (≈350 ms) before it sends the
/// action, so this waits for the menu to close: the pick has happened when it returns.
/// The items of a menu open near the top of one of `windows`: a browser shows a page's pop-up
/// menu in its window, not under the pop-up.
fn window_menu_items(windows: &[AxEl]) -> Vec<AxEl> {
    let mut out = Vec::new();
    let mut level = windows.to_vec();
    for _ in 0..3 {
        let mut next = Vec::new();
        for e in level {
            for c in e.elements("AXChildren") {
                if c.string("AXRole").as_deref() == Some("AXMenu") {
                    out.extend(c.elements("AXChildren"));
                } else {
                    next.push(c);
                }
            }
        }
        level = next;
    }
    out
}

fn pick_popup(el: &AxEl, title: &str, windows: &[AxEl]) -> CuResult<()> {
    let want = norm(title);
    let items = |el: &AxEl| -> Vec<AxEl> {
        el.elements("AXChildren")
            .into_iter()
            .flat_map(|m| m.elements("AXChildren"))
            .collect()
    };
    // A web page's options carry their text as a value, and a closed one keeps only its
    // choice. An open menu's own (titled) item is pressed rather than an option, so the menu
    // closes; a browser shows it in the window a little after the options appear.
    // Only a page's pop-up looks for its menu in the window, or needs a second press: an
    // app's own pop-up blinks the chosen item (≈350 ms) before its menu closes.
    let page = web::in_page(el);
    let titled = |el: &AxEl| {
        let shown = if page {
            window_menu_items(windows)
        } else {
            Vec::new()
        };
        items(el)
            .into_iter()
            .chain(shown)
            .find(|i| i.string("AXTitle").is_some_and(|t| norm(&t) == want))
    };
    let valued = |el: &AxEl| {
        items(el)
            .into_iter()
            .find(|i| i.string("AXValue").is_some_and(|t| norm(&t) == want))
    };
    let mut found = titled(el);
    let opened = found.is_none();
    if opened {
        el.perform("AXPress")?;
        // The app fills the menu when it handles the press, a few milliseconds later.
        let until = Instant::now() + Duration::from_millis(1000);
        while found.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
            found = titled(el).or_else(|| valued(el));
        }
    }
    let closed =
        |el: &AxEl| el.elements("AXChildren").is_empty() || el.bool("AXExpanded") == Some(false);
    match found {
        Some(item) => {
            item.perform("AXPress")?;
            let until = Instant::now() + Duration::from_secs(2);
            let mut menu_pressed = false;
            while opened && !closed(el) {
                if Instant::now() > until {
                    return err(ErrorCode::Failed, "the pop-up's menu didn't close");
                }
                // A page's option, once chosen, may leave the browser's menu open: pressing
                // the pop-up again closes it.
                if page && !menu_pressed && Instant::now() + Duration::from_millis(1850) > until {
                    el.perform("AXPress")?;
                    menu_pressed = true;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        }
        None => {
            if opened {
                for menu in el.elements("AXChildren") {
                    let _ = menu.perform("AXCancel");
                }
            }
            err(
                ErrorCode::NotSettable,
                format!("the pop-up has no item {title:?}"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_target_resolves_to_the_app_the_system_would_run() {
        let by_name = resolve(Some("TextEdit"), None).unwrap();
        assert_eq!(by_name.bundle_id.as_deref(), Some("com.apple.TextEdit"));
        let by_id = resolve(Some("com.apple.TextEdit"), None).unwrap();
        assert_eq!(by_id.bundle_path, by_name.bundle_path);
        let path = by_name.bundle_path.clone().unwrap();
        let by_path = resolve(Some(&path), None).unwrap();
        assert_eq!(by_path.bundle_id.as_deref(), Some("com.apple.TextEdit"));
        let keychain = resolve(Some("Keychain Access"), None).unwrap();
        assert_eq!(
            keychain.bundle_id.as_deref(),
            Some("com.apple.keychainaccess")
        );
        assert!(resolve(Some("No Such App Anywhere"), None).is_none());
    }

    #[test]
    fn selections_count_characters_where_accessibility_counts_utf16_units() {
        let text = "a😀b";
        assert_eq!(to_utf16(text, 1, 1), (1, 2));
        assert_eq!(to_utf16(text, 2, 1), (3, 1));
        assert_eq!(to_utf16(text, 0, 9), (0, 4));
        assert_eq!(to_chars(text, 1, 2), (1, 1));
        assert_eq!(to_chars(text, 3, 1), (2, 1));
        assert_eq!(to_chars(text, 4, 0), (3, 0));
        for (start, length) in [(0, 3), (1, 1), (2, 0)] {
            let (s, l) = to_utf16(text, start, length);
            assert_eq!(to_chars(text, s, l), (start, length));
        }
    }

    #[test]
    fn a_process_start_time_is_read_and_stays_the_same() {
        let me = std::process::id() as i32;
        let started = process_start_us(me).expect("this process has a start time");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        assert!(started <= now && now - started < 24 * 3600 * 1_000_000);
        assert_eq!(process_start_us(me), Some(started));
        assert_eq!(process_start_us(-1), None);
    }
}
