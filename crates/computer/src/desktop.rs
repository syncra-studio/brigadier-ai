//! The seam between the engine and an operating system (§4.1, §6). One implementation per OS;
//! the engine, the renderer and the tool surface don't change between them.

use std::hash::Hash;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use std::sync::Arc;

use crate::cancel::{CancelToken, InputGuard, Release};
use crate::error::{CuResult, ErrorCode, err};
use crate::geom::{ImageTransform, Point, Rect};
use crate::redact::Rgba;
use crate::tree::RawNode;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u32,
    pub pid: i32,
    pub title: String,
    /// Global points, top left of the main display.
    pub frame: Rect,
    pub on_screen: bool,
    pub minimized: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppInfo {
    pub pid: i32,
    pub name: String,
    pub bundle_id: Option<String>,
    pub bundle_path: Option<String>,
    pub frontmost: bool,
    pub windows: Vec<WindowInfo>,
}

/// What a backend can do; anything else returns `unsupported_capability`.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Capabilities {
    pub structure: bool,
    pub capture: bool,
    pub element_actions: bool,
    pub background_keys: bool,
    pub background_pointer: bool,
    /// Making a background app believe it is active without changing the user's focus.
    pub synthetic_activation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Button {
    #[default]
    Left,
    Right,
    Middle,
}

/// Modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Mods {
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// A key with its modifiers, parsed from `cmd+shift+s`, `return`, `a`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub mods: Mods,
    pub key: String,
}

impl Chord {
    pub fn parse(s: &str) -> Option<Self> {
        let mut mods = Mods::default();
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let (key, mod_parts) = parts.split_last()?;
        for m in mod_parts {
            match m.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "meta" | "super" => mods.cmd = true,
                "shift" => mods.shift = true,
                "alt" | "option" | "opt" => mods.alt = true,
                "ctrl" | "control" => mods.ctrl = true,
                _ => return None,
            }
        }
        if key.is_empty() {
            return None;
        }
        Some(Self {
            mods,
            key: key.to_ascii_lowercase(),
        })
    }
}

/// The element that has keyboard focus in an app.
#[derive(Debug, Clone)]
pub struct Focus<E> {
    pub element: Option<E>,
    pub window: Option<u32>,
    pub secure: bool,
    pub role: Option<String>,
    /// The focused element's selected text; never read from a password field.
    pub selected_text: Option<String>,
}

/// The user's side of the desktop, compared before and after every action (F1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UserFocus {
    pub frontmost_pid: i32,
    /// The frontmost app's focused window, by title.
    pub frontmost_window: Option<String>,
    /// The same window's id, when the system can tell.
    pub frontmost_window_id: Option<u32>,
    pub cursor: Point,
    /// The window server's own front process, when the system exposes it.
    pub server_front: Option<u64>,
}

/// One captured image with how it was made.
pub struct Capture {
    pub image: Rgba,
    pub transform: ImageTransform,
}

pub trait Desktop {
    type Element: Clone + Eq + Hash + std::fmt::Debug;

    /// Releases held input; independent of the backend's borrow so a guard can outlive a call.
    fn releaser(&self) -> Arc<dyn Release + Send + Sync>;

    fn capabilities(&self) -> Capabilities;
    fn apps(&mut self) -> CuResult<Vec<AppInfo>>;
    /// One app's windows, read fresh.
    fn windows(&mut self, pid: i32) -> CuResult<Vec<WindowInfo>>;
    /// One window, read fresh.
    fn window(&mut self, id: u32) -> CuResult<WindowInfo>;
    /// The app that owns `pid`, without its windows.
    fn app(&mut self, pid: i32) -> CuResult<AppInfo>;
    /// The window's elements in pre-order, frames in window points. Unless `all` is asked for,
    /// a backend may skip reading rows its lists report out of view; those come back
    /// `unread`, with only their role.
    fn tree(&mut self, window: &WindowInfo, all: bool) -> CuResult<Vec<RawNode<Self::Element>>>;
    /// One element read fresh (children not included), frame in window points.
    fn read(&mut self, window: &WindowInfo, el: &Self::Element)
    -> CuResult<RawNode<Self::Element>>;
    /// The display's pixels per point under the window.
    fn backing_scale(&mut self, window: &WindowInfo) -> f64;
    /// Captures `crop` (window points) at most `pixels_per_point`, at most `max_side` px a side.
    fn capture(
        &mut self,
        window: &WindowInfo,
        crop: Rect,
        pixels_per_point: f64,
        max_side: u32,
    ) -> CuResult<Capture>;

    /// A platform-neutral element action: `press`, `show-menu`, `increment`, `decrement`,
    /// `confirm`, `cancel`, `raise`, `pick`.
    fn perform(&mut self, el: &Self::Element, action: &str) -> CuResult<()>;
    fn set_value(&mut self, el: &Self::Element, text: &str) -> CuResult<()>;
    /// Inserts text at the element's selection, without key events.
    fn insert_text(&mut self, el: &Self::Element, text: &str) -> CuResult<()>;
    fn set_focus(&mut self, el: &Self::Element) -> CuResult<()>;
    /// Sets the element's selected text range, in characters.
    fn select(&mut self, el: &Self::Element, start: usize, length: usize) -> CuResult<()>;
    /// The element's selected text range, in characters.
    fn selection(&mut self, el: &Self::Element) -> Option<(usize, usize)>;
    /// Presses the menu-bar item at `path`, e.g. `["File", "Save As…"]`.
    fn menu(&mut self, pid: i32, path: &[String]) -> CuResult<()>;
    fn focus(&mut self, pid: i32) -> CuResult<Focus<Self::Element>>;

    /// A click at a window point. `activate` wraps it in synthetic activation.
    #[allow(clippy::too_many_arguments)]
    fn click(
        &mut self,
        w: &WindowInfo,
        at: Point,
        button: Button,
        count: u8,
        mods: Mods,
        activate: bool,
        guard: &mut InputGuard<'_>,
    ) -> CuResult<()>;
    fn scroll(&mut self, w: &WindowInfo, at: Point, dx: i32, dy: i32) -> CuResult<()>;
    fn drag(
        &mut self,
        w: &WindowInfo,
        from: Point,
        to: Point,
        activate: bool,
        guard: &mut InputGuard<'_>,
        cancel: &CancelToken,
    ) -> CuResult<()>;
    fn key(&mut self, pid: i32, chord: &Chord, guard: &mut InputGuard<'_>) -> CuResult<()>;
    fn type_text(&mut self, pid: i32, text: &str, cancel: &CancelToken) -> CuResult<()>;

    /// Starts listening for the app's accessibility notifications.
    fn watch(&mut self, pid: i32);
    /// Runs the notification loop for at most `d`.
    fn pump(&mut self, d: Duration);
    /// When the app last posted a notification.
    fn last_notification(&self, pid: i32) -> Option<Instant>;

    fn user_focus(&mut self) -> UserFocus;

    /// Reads, from any thread, how many seconds ago the user last used a mouse, trackpad or
    /// keyboard. Input this crate posts doesn't count. Zero when the backend can't tell, which
    /// keeps the foreground rung off.
    fn idle_source(&self) -> Arc<dyn Fn() -> f64 + Send + Sync> {
        Arc::new(|| 0.0)
    }
    /// The foreground rung: shows the window if it's minimised, raises it and brings its app to
    /// the front.
    fn raise(&mut self, w: &WindowInfo) -> CuResult<()> {
        let _ = w;
        err(ErrorCode::UnsupportedCapability, "raising a window")
    }
    /// Brings an app to the front: the user's own, given back after the foreground rung or a
    /// launch that took the front.
    fn activate(&mut self, pid: i32) -> CuResult<()> {
        let _ = pid;
        err(ErrorCode::UnsupportedCapability, "activating an app")
    }
    fn minimize(&mut self, w: &WindowInfo) -> CuResult<()> {
        let _ = w;
        err(ErrorCode::UnsupportedCapability, "minimising a window")
    }
    /// The file or URL the window shows, as the app reports it (a `file:` URL for a file);
    /// `None` when it doesn't say.
    fn document(&mut self, w: &WindowInfo) -> Option<String> {
        let _ = w;
        None
    }
    /// Closes the window as its close button would: an app may keep it open to ask about
    /// unsaved changes.
    fn close(&mut self, w: &WindowInfo) -> CuResult<()> {
        let _ = w;
        err(ErrorCode::UnsupportedCapability, "closing a window")
    }
    /// Opens an app, a file or a URL (or a file or URL in an app) without bringing it to the
    /// front, and returns once the system took the request; windows come later.
    fn open(&mut self, app: Option<&str>, target: Option<&str>) -> CuResult<()> {
        let _ = (app, target);
        err(ErrorCode::UnsupportedCapability, "launching")
    }
}
