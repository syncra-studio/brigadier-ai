//! The macOS backend (§4.1): accessibility for structure and element actions,
//! ScreenCaptureKit for pixels, pid-posted events for background input.
//!
//! The system APIs are C and Objective-C, so this module is where the crate's unsafe code
//! lives; every block says why it holds.
#![allow(unsafe_code)]

mod ax;
mod capture;
mod input;
pub mod private;

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
    CGPoint as CgPoint, kCFRunLoopDefaultMode,
};
use objc2_core_graphics::{
    CGDisplayBounds, CGDisplayCopyDisplayMode, CGDisplayMode, CGEvent, CGGetActiveDisplayList,
    CGGetDisplaysWithPoint, CGWindowListCopyWindowInfo, CGWindowListOption, kCGWindowBounds,
    kCGWindowIsOnscreen, kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerPID,
};

pub use ax::AxEl;
use private::Private;

use crate::cancel::{CancelToken, InputGuard, Release};
use crate::desktop::{
    AppInfo, Button, Capabilities, Capture, Chord, Desktop, Focus, Mods, UserFocus, WindowInfo,
};
use crate::error::{CuError, CuResult, ErrorCode, err};
use crate::geom::{Point, Rect};
use crate::tree::RawNode;

/// Whether this process may use accessibility and capture the screen.
pub fn permissions() -> (bool, bool) {
    // SAFETY: both are plain queries.
    unsafe {
        (
            objc2_application_services::AXIsProcessTrusted(),
            objc2_core_graphics::CGPreflightScreenCaptureAccess(),
        )
    }
}

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
        })
    }

    fn ax_window(&mut self, w: &WindowInfo) -> CuResult<AxEl> {
        if let Some(el) = self.ax_windows.get(&w.id) {
            return Ok(el.clone());
        }
        for el in AxEl::app(w.pid).elements("AXWindows") {
            if let Some(id) = el.window_id() {
                self.ax_windows.insert(id, el);
            }
        }
        self.ax_windows.get(&w.id).cloned().ok_or_else(|| {
            CuError::new(
                ErrorCode::NoSuchTarget,
                format!("window w{} has no accessibility element", w.id),
            )
        })
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

impl Desktop for MacDesktop {
    type Element = AxEl;

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
        let ws = NSWorkspace::sharedWorkspace();
        let front = ws.frontmostApplication().map(|a| a.processIdentifier());
        let windows = Self::window_list(
            CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements,
            0,
        );
        let mut out = Vec::new();
        for app in ws.runningApplications().iter() {
            let mut info = Self::app_info(&app, front);
            info.windows = windows
                .iter()
                .filter(|w| w.pid == info.pid)
                .cloned()
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
        Ok(all.into_iter().filter(|w| w.pid == pid).collect())
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
        if !w.on_screen
            && let Ok(el) = self.ax_window(&w)
        {
            w.minimized = el.bool("AXMinimized").unwrap_or(false);
        }
        Ok(w)
    }

    fn app(&mut self, pid: i32) -> CuResult<AppInfo> {
        let front = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier());
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .map(|a| Self::app_info(&a, front))
            .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, format!("no app with pid {pid}")))
    }

    fn tree(&mut self, w: &WindowInfo, all: bool) -> CuResult<Vec<RawNode<AxEl>>> {
        let el = self.ax_window(w)?;
        let nodes = ax::tree(&el, Point::new(w.frame.x, w.frame.y), all);
        if nodes.len() <= 1 && el.attr("AXRole").is_err() {
            self.ax_windows.remove(&w.id);
            return err(
                ErrorCode::StaleRef,
                "the window's accessibility element went away; observe again",
            );
        }
        Ok(nodes)
    }

    fn read(&mut self, w: &WindowInfo, el: &AxEl) -> CuResult<RawNode<AxEl>> {
        ax::read(el, Point::new(w.frame.x, w.frame.y))
    }

    fn backing_scale(&mut self, w: &WindowInfo) -> f64 {
        let c = w.frame.center();
        let mut ids = [0u32; 4];
        let mut n = 0u32;
        // SAFETY: `ids` has room for 4 displays and `n` is a valid out pointer.
        let ok =
            unsafe { CGGetDisplaysWithPoint(CgPoint::new(c.x, c.y), 4, ids.as_mut_ptr(), &mut n) };
        if ok.0 != 0 || n == 0 {
            return 2.0;
        }
        let Some(mode) = CGDisplayCopyDisplayMode(ids[0]) else {
            return 2.0;
        };
        let (px, pt) = (
            CGDisplayMode::pixel_width(Some(&mode)),
            CGDisplayMode::width(Some(&mode)),
        );
        if pt == 0 { 2.0 } else { px as f64 / pt as f64 }
    }

    fn capture(
        &mut self,
        w: &WindowInfo,
        crop: Rect,
        pixels_per_point: f64,
        max_side: u32,
    ) -> CuResult<Capture> {
        let (image, transform) =
            self.shareable
                .capture(w.id, w.frame, crop, pixels_per_point, max_side)?;
        Ok(Capture { image, transform })
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
            return pick_popup(el, text);
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

    fn menu(&mut self, pid: i32, path: &[String]) -> CuResult<()> {
        let Some(bar) = AxEl::app(pid).element("AXMenuBar") else {
            return err(ErrorCode::NoSuchTarget, "the app has no menu bar");
        };
        let mut level = bar.elements("AXChildren");
        for (i, name) in path.iter().enumerate() {
            let want = norm(name);
            let Some(item) = level
                .into_iter()
                .find(|e| e.string("AXTitle").is_some_and(|t| norm(&t) == want))
            else {
                return err(ErrorCode::NoSuchTarget, format!("no menu item {name:?}"));
            };
            if i + 1 == path.len() {
                if item.bool("AXEnabled") == Some(false) {
                    return err(ErrorCode::Failed, format!("menu item {name:?} is disabled"));
                }
                return item.perform("AXPress");
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
        Ok(Focus {
            element: el,
            window,
            secure,
            role,
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
        input::click(w, at, button, count, mods, activate, guard)
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
        input::drag(w, from, to, activate, guard, cancel)
    }

    fn key(&mut self, pid: i32, chord: &Chord, guard: &mut InputGuard<'_>) -> CuResult<()> {
        input::key(pid, chord, guard)
    }

    fn type_text(&mut self, pid: i32, text: &str, cancel: &CancelToken) -> CuResult<()> {
        input::type_text(pid, text, cancel)
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

    fn user_focus(&mut self) -> UserFocus {
        let front = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|a| a.processIdentifier())
            .unwrap_or(0);
        let frontmost_window = (front != 0)
            .then(|| AxEl::app(front).element("AXFocusedWindow"))
            .flatten()
            .and_then(|w| w.string("AXTitle"));
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
            cursor,
            server_front,
        }
    }
}

/// The active displays' bounds, in global points.
fn display_bounds() -> Vec<Rect> {
    let mut ids = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: both pointers are valid for the sizes given.
    unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) };
    ids[..(n as usize).min(ids.len())]
        .iter()
        .map(|&id| {
            let b = CGDisplayBounds(id);
            Rect::new(b.origin.x, b.origin.y, b.size.width, b.size.height)
        })
        .collect()
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
fn pick_popup(el: &AxEl, title: &str) -> CuResult<()> {
    let want = norm(title);
    let items = |el: &AxEl| -> Vec<AxEl> {
        el.elements("AXChildren")
            .into_iter()
            .flat_map(|m| m.elements("AXChildren"))
            .collect()
    };
    let mut found = items(el);
    let opened = found.is_empty();
    if opened {
        el.perform("AXPress")?;
        // The app fills the menu when it handles the press, a few milliseconds later.
        let until = Instant::now() + Duration::from_millis(500);
        found = items(el);
        while found.is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(1));
            found = items(el);
        }
    }
    match found
        .into_iter()
        .find(|i| i.string("AXTitle").is_some_and(|t| norm(&t) == want))
    {
        Some(item) => {
            item.perform("AXPress")?;
            let until = Instant::now() + Duration::from_secs(2);
            while opened && !el.elements("AXChildren").is_empty() {
                if Instant::now() > until {
                    return err(ErrorCode::Failed, "the pop-up's menu didn't close");
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
