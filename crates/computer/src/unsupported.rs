//! The backend for systems without one yet (§6): every call says so, so the crate and its tool
//! surface build and answer everywhere.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::cancel::{CancelToken, Held, InputGuard, Release};
use crate::desktop::{
    AppInfo, Button, Capabilities, Capture, Chord, Desktop, Focus, Mods, UserFocus, WindowInfo,
};
use crate::error::{CuResult, ErrorCode, err};
use crate::geom::{Point, Rect};
use crate::tree::RawNode;

pub struct Unsupported;

struct NoRelease;
impl Release for NoRelease {
    fn release(&self, _: Held) {}
}

fn no<T>() -> CuResult<T> {
    err(
        ErrorCode::UnsupportedCapability,
        "computer use isn't available on this system yet",
    )
}

impl Desktop for Unsupported {
    type Element = u64;

    fn releaser(&self) -> Arc<dyn Release + Send + Sync> {
        Arc::new(NoRelease)
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn apps(&mut self) -> CuResult<Vec<AppInfo>> {
        no()
    }
    fn windows(&mut self, _: i32) -> CuResult<Vec<WindowInfo>> {
        no()
    }
    fn window(&mut self, _: u32) -> CuResult<WindowInfo> {
        no()
    }
    fn app(&mut self, _: i32) -> CuResult<AppInfo> {
        no()
    }
    fn tree(&mut self, _: &WindowInfo) -> CuResult<Vec<RawNode<u64>>> {
        no()
    }
    fn read(&mut self, _: &WindowInfo, _: &u64) -> CuResult<RawNode<u64>> {
        no()
    }
    fn backing_scale(&mut self, _: &WindowInfo) -> f64 {
        1.0
    }
    fn capture(&mut self, _: &WindowInfo, _: Rect, _: f64, _: u32) -> CuResult<Capture> {
        no()
    }
    fn perform(&mut self, _: &u64, _: &str) -> CuResult<()> {
        no()
    }
    fn set_value(&mut self, _: &u64, _: &str) -> CuResult<()> {
        no()
    }
    fn insert_text(&mut self, _: &u64, _: &str) -> CuResult<()> {
        no()
    }
    fn set_focus(&mut self, _: &u64) -> CuResult<()> {
        no()
    }
    fn menu(&mut self, _: i32, _: &[String]) -> CuResult<()> {
        no()
    }
    fn focus(&mut self, _: i32) -> CuResult<Focus<u64>> {
        no()
    }
    fn click(
        &mut self,
        _: &WindowInfo,
        _: Point,
        _: Button,
        _: u8,
        _: Mods,
        _: bool,
        _: &mut InputGuard<'_>,
    ) -> CuResult<()> {
        no()
    }
    fn scroll(&mut self, _: &WindowInfo, _: Point, _: i32, _: i32) -> CuResult<()> {
        no()
    }
    fn drag(
        &mut self,
        _: &WindowInfo,
        _: Point,
        _: Point,
        _: bool,
        _: &mut InputGuard<'_>,
        _: &CancelToken,
    ) -> CuResult<()> {
        no()
    }
    fn key(&mut self, _: i32, _: &Chord, _: &mut InputGuard<'_>) -> CuResult<()> {
        no()
    }
    fn type_text(&mut self, _: i32, _: &str, _: &CancelToken) -> CuResult<()> {
        no()
    }
    fn watch(&mut self, _: i32) {}
    fn pump(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn last_notification(&self, _: i32) -> Option<Instant> {
        None
    }
    fn user_focus(&mut self) -> UserFocus {
        UserFocus {
            frontmost_pid: 0,
            frontmost_window: None,
            cursor: Point::new(0.0, 0.0),
            server_front: None,
        }
    }
}
