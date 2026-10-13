//! Keeps a desktop web app's window drawn while the user's windows cover it.
//!
//! An app learns that its window is covered from the window server's
//! occlusion notifications. Chromium and Electron then mark the page hidden:
//! it stops drawing and its timers slow down, so what an agent reads or
//! captures goes stale while the user works on top of it. Turning those
//! notifications off for one window while it is still at least partly
//! visible pins the app's view at "visible": the page keeps running when the
//! window is covered later. A window that is already covered (or on another
//! Space) is left alone, because pinning it would pin it hidden.
//!
//! A window is pinned when an agent reads it, and released (its previous
//! setting put back) when the last agent session that read it ends, when
//! the helper exits, or dropped when the window is gone. Nothing here
//! activates, raises or moves a window. The setting belongs to the helper's
//! window-server connection, so it also ends if the helper crashes.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{ProtectedResourceOwnership, Tool, ToolDef, ToolRegistry};
use serde_json::Value;

/// The window server's side of keeping a window drawn. Tests supply a fake.
pub trait Occlusion: Send + Sync {
    /// Whether `pid`'s app draws its pages with Chromium (browsers and
    /// Electron apps).
    fn web_engine(&self, pid: i32) -> bool;
    /// Whether the window is on screen with some part not covered by
    /// another app's window.
    fn partly_visible(&self, window_id: u32) -> bool;
    /// Whether the window still exists.
    fn exists(&self, window_id: u32) -> bool;
    /// Turn the window's occlusion notifications on or off. Returns the
    /// previous setting, `None` when the window server refused.
    fn set_notifications(&self, window_id: u32, enabled: bool) -> Option<bool>;
}

struct Pin {
    /// The notifications setting to put back on release.
    previous: bool,
    /// Agent sessions that read the window since it was pinned.
    owners: HashSet<String>,
}

/// The windows kept drawn, with who keeps them.
pub struct KeepDrawn {
    occlusion: Box<dyn Occlusion>,
    pins: Mutex<HashMap<u32, Pin>>,
}

impl KeepDrawn {
    pub fn new(occlusion: Box<dyn Occlusion>) -> Self {
        Self {
            occlusion,
            pins: Mutex::new(HashMap::new()),
        }
    }

    /// Keep `window_id` of `pid` drawn for `owner`. `true` when it is kept
    /// (now or already); `false` when it isn't a web app's window, is
    /// already covered, or the window server refused.
    pub fn keep(&self, pid: i32, window_id: u32, owner: &str) -> bool {
        let mut pins = self.pins.lock().unwrap_or_else(|e| e.into_inner());
        pins.retain(|window, _| *window == window_id || self.occlusion.exists(*window));
        if let Some(pin) = pins.get_mut(&window_id) {
            pin.owners.insert(owner.to_owned());
            return true;
        }
        if !self.occlusion.web_engine(pid) || !self.occlusion.partly_visible(window_id) {
            return false;
        }
        let Some(previous) = self.occlusion.set_notifications(window_id, false) else {
            return false;
        };
        pins.insert(
            window_id,
            Pin {
                previous,
                owners: HashSet::from([owner.to_owned()]),
            },
        );
        true
    }

    /// `owner` no longer needs its windows drawn; windows nobody else keeps
    /// get their previous setting back.
    pub fn release(&self, owner: &str) {
        let mut pins = self.pins.lock().unwrap_or_else(|e| e.into_inner());
        pins.retain(|window, pin| {
            pin.owners.remove(owner);
            !pin.owners.is_empty() || !self.restore(*window, pin.previous)
        });
    }

    /// Put every kept window's previous setting back.
    pub fn release_all(&self) {
        let mut pins = self.pins.lock().unwrap_or_else(|e| e.into_inner());
        pins.retain(|window, pin| !self.restore(*window, pin.previous));
    }

    /// Whether `window_id` is kept drawn.
    pub fn kept(&self, window_id: u32) -> bool {
        let pins = self.pins.lock().unwrap_or_else(|e| e.into_inner());
        pins.contains_key(&window_id)
    }

    /// `true` when the window has its setting back or is gone, so the pin
    /// can be dropped; a refused restore is retried on the next release.
    fn restore(&self, window_id: u32, previous: bool) -> bool {
        !self.occlusion.exists(window_id)
            || self
                .occlusion
                .set_notifications(window_id, previous)
                .is_some()
    }
}

/// The driver's `get_window_state` that keeps a web app's window drawn once
/// an agent has read it.
pub struct KeepDrawnRead {
    inner: Box<dyn Tool>,
    keep: &'static KeepDrawn,
}

/// Wrap the driver's `get_window_state` so the windows it reads are kept
/// drawn, and release a session's windows when it ends. Call before
/// [`crate::guard::install`].
#[cfg(target_os = "macos")]
pub fn install(registry: &mut ToolRegistry) {
    install_with(registry, system());
}

/// [`install`] with the given windows.
pub fn install_with(registry: &mut ToolRegistry, keep: &'static KeepDrawn) {
    registry.wrap("get_window_state", move |inner| {
        Box::new(KeepDrawnRead { inner, keep }) as Box<dyn Tool>
    });
    let hook = cua_driver_core::session::register_scoped_fallible_session_end_hook(
        "keep_drawn",
        move |session| {
            keep.release(session);
            Ok(())
        },
    );
    registry.retain_session_end_hook(hook);
}

/// The window and app a read addressed, from its answer, else its arguments.
fn read_target(args: &Value, result: &ToolResult) -> Option<(i32, u32)> {
    let from = |value: Option<&Value>, key: &str| value.and_then(|v| v.get(key)?.as_i64());
    let structured = result.structured_content.as_ref();
    let pid = from(structured, "pid").or_else(|| from(Some(args), "pid"))?;
    let window = from(structured, "window_id").or_else(|| from(Some(args), "window_id"))?;
    Some((i32::try_from(pid).ok()?, u32::try_from(window).ok()?))
}

#[async_trait]
impl Tool for KeepDrawnRead {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    fn has_independent_input_lane(&self, args: &Value) -> bool {
        self.inner.has_independent_input_lane(args)
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        self.inner
            .protected_resource_ownership(adapter_id, args)
            .await
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        self.inner.protected_resource_scope(adapter_id, args).await
    }

    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), String> {
        self.inner
            .validate_protected_resource_scope(adapter_id, args, approved_scope)
            .await
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let result = self.inner.invoke(args.clone()).await;
        if result.is_error == Some(true) {
            return result;
        }
        if let Some((pid, window_id)) = read_target(&args, &result) {
            let owner = crate::scope::current().session.unwrap_or_default();
            let keep = self.keep;
            let _ = tokio::task::spawn_blocking(move || keep.keep(pid, window_id, &owner)).await;
        }
        result
    }
}

/// The process-wide windows kept drawn, released when the process exits.
#[cfg(target_os = "macos")]
pub fn system() -> &'static KeepDrawn {
    static KEEP: OnceLock<KeepDrawn> = OnceLock::new();
    KEEP.get_or_init(|| {
        extern "C" fn release_at_exit() {
            if let Some(keep) = KEEP.get() {
                keep.release_all();
            }
        }
        // SAFETY: registers a plain function; it only takes a lock.
        unsafe { libc::atexit(release_at_exit) };
        KeepDrawn::new(Box::new(system::WindowServer))
    })
}

#[cfg(target_os = "macos")]
mod system {
    use std::ffi::c_char;
    use std::sync::OnceLock;

    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
    }

    /// `kCGWindowListOptionOnScreenAboveWindow`.
    const ON_SCREEN_ABOVE_WINDOW: u32 = 1 << 1;
    /// `kCGWindowListOptionIncludingWindow`.
    const INCLUDING_WINDOW: u32 = 1 << 3;
    /// `kCGWindowListExcludeDesktopElements`.
    const EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
    /// Sample points per side when looking for an uncovered part.
    const GRID: usize = 16;

    /// `CGError SLSPackagesEnableWindowOcclusionNotifications(CGSConnectionID,
    /// CGWindowID, bool enable, uint8_t *previous)`.
    type EnableNotifications = unsafe extern "C" fn(u32, u32, u8, *mut u8) -> i32;

    fn enable_notifications() -> Option<EnableNotifications> {
        static SYM: OnceLock<Option<usize>> = OnceLock::new();
        let ptr = *SYM.get_or_init(|| {
            // Loads the window server's private framework the symbol lives in.
            platform_macos::input::skylight::main_connection_id()?;
            // SAFETY: a symbol lookup in the already loaded images.
            let ptr = unsafe {
                libc::dlsym(
                    libc::RTLD_DEFAULT,
                    c"SLSPackagesEnableWindowOcclusionNotifications".as_ptr() as *const c_char,
                )
            };
            (!ptr.is_null()).then_some(ptr as usize)
        });
        // SAFETY: the symbol has this signature on every macOS that ships it.
        ptr.map(|ptr| unsafe { std::mem::transmute::<usize, EnableNotifications>(ptr) })
    }

    #[derive(Clone, Copy)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    impl Rect {
        fn contains(&self, x: f64, y: f64) -> bool {
            x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
        }
    }

    struct Row {
        pid: i64,
        layer: i64,
        on_screen: bool,
        bounds: Rect,
    }

    fn rows(option: u32, window_id: u32) -> Vec<Row> {
        // SAFETY: the call returns a +1 array (or null), wrapped under the
        // create rule so it is released once.
        let raw = unsafe { CGWindowListCopyWindowInfo(option, window_id) };
        if raw.is_null() {
            return Vec::new();
        }
        let rows: CFArray<CFDictionary<CFString, CFType>> =
            unsafe { CFArray::wrap_under_create_rule(raw) };
        rows.iter()
            .filter_map(|row| {
                let value = |key: &'static str| row.find(CFString::from_static_string(key));
                let number = |key| value(key)?.downcast::<CFNumber>()?.to_i64();
                let bounds = value("kCGWindowBounds")?.downcast::<CFDictionary>()?;
                let side = |key: &'static str| {
                    bounds
                        .find(CFString::from_static_string(key).as_CFTypeRef())
                        .map(|value| unsafe { CFType::wrap_under_get_rule(*value) })
                        .and_then(|value| value.downcast::<CFNumber>())
                        .and_then(|number| number.to_f64())
                };
                Some(Row {
                    pid: number("kCGWindowOwnerPID")?,
                    layer: number("kCGWindowLayer")?,
                    on_screen: value("kCGWindowIsOnscreen")
                        .and_then(|value| value.downcast::<CFBoolean>())
                        .is_some_and(bool::from),
                    bounds: Rect {
                        x: side("X")?,
                        y: side("Y")?,
                        width: side("Width")?,
                        height: side("Height")?,
                    },
                })
            })
            .collect()
    }

    /// Whether some sample point of `bounds` lies outside every covering
    /// rectangle.
    pub(super) fn uncovered_part(
        bounds: (f64, f64, f64, f64),
        covering: &[(f64, f64, f64, f64)],
    ) -> bool {
        let rect = |(x, y, width, height)| Rect {
            x,
            y,
            width,
            height,
        };
        let window = rect(bounds);
        let covering: Vec<Rect> = covering.iter().copied().map(rect).collect();
        if window.width <= 0.0 || window.height <= 0.0 {
            return false;
        }
        (0..GRID * GRID).any(|index| {
            let (row, column) = ((index / GRID) as f64, (index % GRID) as f64);
            let x = window.x + window.width * (column + 0.5) / GRID as f64;
            let y = window.y + window.height * (row + 0.5) / GRID as f64;
            !covering.iter().any(|cover| cover.contains(x, y))
        })
    }

    pub struct WindowServer;

    impl super::Occlusion for WindowServer {
        fn web_engine(&self, pid: i32) -> bool {
            crate::hotkey::chromium_family(pid)
        }

        fn partly_visible(&self, window_id: u32) -> bool {
            let Some(window) = rows(INCLUDING_WINDOW, window_id).into_iter().next() else {
                return false;
            };
            if !window.on_screen {
                return false;
            }
            let own = std::process::id() as i64;
            let covering: Vec<_> =
                rows(ON_SCREEN_ABOVE_WINDOW | EXCLUDE_DESKTOP_ELEMENTS, window_id)
                    .into_iter()
                    .filter(|row| row.layer == 0 && row.pid != own)
                    .map(|row| {
                        (
                            row.bounds.x,
                            row.bounds.y,
                            row.bounds.width,
                            row.bounds.height,
                        )
                    })
                    .collect();
            let b = window.bounds;
            uncovered_part((b.x, b.y, b.width, b.height), &covering)
        }

        fn exists(&self, window_id: u32) -> bool {
            !rows(INCLUDING_WINDOW, window_id).is_empty()
        }

        fn set_notifications(&self, window_id: u32, enabled: bool) -> Option<bool> {
            let enable = enable_notifications()?;
            let connection = platform_macos::input::skylight::main_connection_id()?;
            let mut previous = 0u8;
            // SAFETY: `previous` outlives the call.
            let status = unsafe {
                enable(
                    connection,
                    window_id,
                    u8::from(enabled),
                    &mut previous as *mut u8,
                )
            };
            (status == 0).then_some(previous != 0)
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::system::uncovered_part;

    #[test]
    fn a_window_with_one_uncovered_strip_is_partly_visible() {
        let window = (0.0, 0.0, 100.0, 100.0);
        assert!(uncovered_part(window, &[]));
        assert!(uncovered_part(window, &[(0.0, 0.0, 90.0, 100.0)]));
        assert!(!uncovered_part(window, &[(-10.0, -10.0, 200.0, 200.0)]));
        assert!(!uncovered_part(
            window,
            &[(0.0, 0.0, 50.0, 100.0), (50.0, 0.0, 50.0, 100.0)]
        ));
        assert!(!uncovered_part((0.0, 0.0, 0.0, 10.0), &[]));
    }
}
