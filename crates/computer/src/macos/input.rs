//! Background input: events posted to one app, aimed at one window, never through the
//! system's shared input stream, so the user's cursor and focus stay theirs (§2.1, §4.4).

use std::time::Duration;

use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventType,
    CGMouseButton, CGScrollEventUnit,
};

use super::private::{FIELD_MOUSE_WINDOW, FIELD_MOVED_WINDOW, Private};
use crate::cancel::{CancelToken, Held, InputGuard, Release};
use crate::desktop::{Button, Chord, Mods, WindowInfo};
use crate::error::{CuResult, ErrorCode, err};
use crate::geom::Point;

fn source() -> Option<CFRetained<CGEventSource>> {
    CGEventSource::new(CGEventSourceStateID::Private)
}

fn post(pid: i32, e: &CGEvent) {
    CGEvent::post_to_pid(pid, Some(e));
}

fn cg_button(b: Button) -> CGMouseButton {
    match b {
        Button::Left => CGMouseButton::Left,
        Button::Right => CGMouseButton::Right,
        Button::Middle => CGMouseButton::Center,
    }
}

fn button_types(b: Button) -> (CGEventType, CGEventType, CGEventType) {
    match b {
        Button::Left => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGEventType::LeftMouseDragged,
        ),
        Button::Right => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::RightMouseDragged,
        ),
        Button::Middle => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::OtherMouseDragged,
        ),
    }
}

pub fn flags(m: Mods) -> CGEventFlags {
    let mut f = CGEventFlags(0);
    if m.cmd {
        f.0 |= CGEventFlags::MaskCommand.0;
    }
    if m.shift {
        f.0 |= CGEventFlags::MaskShift.0;
    }
    if m.alt {
        f.0 |= CGEventFlags::MaskAlternate.0;
    }
    if m.ctrl {
        f.0 |= CGEventFlags::MaskControl.0;
    }
    f
}

/// A mouse event for a window point, routed to that window.
fn mouse_event(
    w: &WindowInfo,
    ty: CGEventType,
    local: Point,
    button: Button,
    clicks: i64,
    mods: Mods,
) -> CuResult<CFRetained<CGEvent>> {
    let global = CGPoint::new(w.frame.x + local.x, w.frame.y + local.y);
    let Some(e) = CGEvent::new_mouse_event(source().as_deref(), ty, global, cg_button(button))
    else {
        return err(ErrorCode::Failed, "couldn't make a mouse event");
    };
    CGEvent::set_integer_value_field(Some(&e), CGEventField::MouseEventClickState, clicks);
    let field = if ty == CGEventType::MouseMoved {
        FIELD_MOVED_WINDOW
    } else {
        FIELD_MOUSE_WINDOW
    };
    CGEvent::set_integer_value_field(Some(&e), CGEventField(field), i64::from(w.id));
    CGEvent::set_flags(Some(&e), flags(mods));
    if !Private::get().set_window_location(&e, CGPoint::new(local.x, local.y)) {
        return err(
            ErrorCode::UnsupportedCapability,
            "background clicks need a system call this macOS lacks",
        );
    }
    Ok(e)
}

/// Makes a background app believe it is active and its window key, and undoes it on drop.
/// The window server's front process and the user's key window don't change (§2.1).
pub struct Activation {
    psn: Option<super::private::Psn>,
    window: u32,
}

impl Activation {
    pub fn begin(pid: i32, window: u32, on: bool) -> CuResult<Self> {
        if !on {
            return Ok(Self { psn: None, window });
        }
        let p = Private::get();
        let Some(psn) = p.psn(pid) else {
            return err(
                ErrorCode::UnsupportedCapability,
                "synthetic activation isn't available",
            );
        };
        if !(p.focus_record(&psn, window, true) && p.make_key(&psn, window)) {
            return err(
                ErrorCode::BackgroundUnavailable,
                "the app didn't take synthetic activation",
            );
        }
        Ok(Self {
            psn: Some(psn),
            window,
        })
    }
}

impl Drop for Activation {
    fn drop(&mut self) {
        if let Some(psn) = &self.psn {
            // Queued behind the events of the action, so the app handles them first.
            Private::get().focus_record(psn, self.window, false);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn click(
    w: &WindowInfo,
    at: Point,
    button: Button,
    count: u8,
    mods: Mods,
    activate: bool,
    guard: &mut InputGuard<'_>,
) -> CuResult<()> {
    let (down, up, _) = button_types(button);
    let _act = Activation::begin(w.pid, w.id, activate)?;
    for n in 1..=i64::from(count.max(1)) {
        let d = mouse_event(w, down, at, button, n, mods)?;
        let u = mouse_event(w, up, at, button, n, mods)?;
        let held = Held::Button {
            pid: w.pid,
            window: w.id,
            button: button as u8,
        };
        post(w.pid, &d);
        guard.pressed(held);
        post(w.pid, &u);
        guard.released(held);
    }
    Ok(())
}

pub fn drag(
    w: &WindowInfo,
    from: Point,
    to: Point,
    activate: bool,
    guard: &mut InputGuard<'_>,
    cancel: &CancelToken,
) -> CuResult<()> {
    let (down, up, dragged) = button_types(Button::Left);
    let _act = Activation::begin(w.pid, w.id, activate)?;
    let held = Held::Button {
        pid: w.pid,
        window: w.id,
        button: 0,
    };
    let e = mouse_event(w, down, from, Button::Left, 1, Mods::default())?;
    post(w.pid, &e);
    guard.pressed(held);
    // About one step per 8 points, at least 8 steps, paced like a hand.
    let dist = ((to.x - from.x).powi(2) + (to.y - from.y).powi(2)).sqrt();
    let steps = ((dist / 8.0).ceil() as u32).clamp(8, 60);
    for i in 1..=steps {
        cancel.check()?;
        let f = f64::from(i) / f64::from(steps);
        let p = Point::new(from.x + (to.x - from.x) * f, from.y + (to.y - from.y) * f);
        let e = mouse_event(w, dragged, p, Button::Left, 1, Mods::default())?;
        post(w.pid, &e);
        std::thread::sleep(Duration::from_millis(4));
    }
    let e = mouse_event(w, up, to, Button::Left, 1, Mods::default())?;
    post(w.pid, &e);
    guard.released(held);
    Ok(())
}

pub fn scroll(w: &WindowInfo, at: Point, dx: i32, dy: i32) -> CuResult<()> {
    // Lines: positive dy scrolls content down (towards the end), like a wheel turned towards
    // the user. The event's sign is the other way round.
    let Some(e) = CGEvent::new_scroll_wheel_event2(
        source().as_deref(),
        CGScrollEventUnit::Line,
        2,
        -dy,
        -dx,
        0,
    ) else {
        return err(ErrorCode::Failed, "couldn't make a scroll event");
    };
    CGEvent::set_location(Some(&e), CGPoint::new(w.frame.x + at.x, w.frame.y + at.y));
    CGEvent::set_integer_value_field(Some(&e), CGEventField(FIELD_MOUSE_WINDOW), i64::from(w.id));
    if !Private::get().set_window_location(&e, CGPoint::new(at.x, at.y)) {
        return err(
            ErrorCode::UnsupportedCapability,
            "background scrolling needs a system call this macOS lacks",
        );
    }
    post(w.pid, &e);
    Ok(())
}

/// The virtual key code of a key name, on the ANSI layout.
pub fn keycode(name: &str) -> Option<u16> {
    Some(match name {
        "return" | "enter" => 36,
        "tab" => 48,
        "space" => 49,
        "delete" | "backspace" => 51,
        "escape" | "esc" => 53,
        "forwarddelete" | "del" => 117,
        "left" => 123,
        "right" => 124,
        "down" => 125,
        "up" => 126,
        "home" => 115,
        "end" => 119,
        "pageup" => 116,
        "pagedown" => 121,
        "f1" => 122,
        "f2" => 120,
        "f3" => 99,
        "f4" => 118,
        "f5" => 96,
        "f6" => 97,
        "f7" => 98,
        "f8" => 100,
        "f9" => 101,
        "f10" => 109,
        "f11" => 103,
        "f12" => 111,
        k if k.chars().count() == 1 => {
            const ANSI: &str = "asdfhgzxcv\0bqweryt123465=97-80]ou[ip\0lj'k;\\,/nm.\0\0`";
            let c = k.chars().next()?;
            ANSI.chars().position(|a| a == c)? as u16
        }
        _ => return None,
    })
}

pub fn key(pid: i32, chord: &Chord, _guard: &mut InputGuard<'_>) -> CuResult<()> {
    let Some(code) = keycode(&chord.key) else {
        return err(
            ErrorCode::BadRequest,
            format!("unknown key {:?}", chord.key),
        );
    };
    // Modifiers travel as the events' flags, so no modifier key is ever held down.
    for down in [true, false] {
        let Some(e) = CGEvent::new_keyboard_event(source().as_deref(), code, down) else {
            return err(ErrorCode::Failed, "couldn't make a key event");
        };
        CGEvent::set_flags(Some(&e), flags(chord.mods));
        post(pid, &e);
    }
    Ok(())
}

/// Characters per key event: the system takes up to 20 UTF-16 units in one.
const CHUNK: usize = 16;

pub fn type_text(pid: i32, text: &str, cancel: &CancelToken) -> CuResult<()> {
    let mut guard_free = InputGuard::new(&NoRelease);
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            key(
                pid,
                &Chord {
                    mods: Mods::default(),
                    key: "return".into(),
                },
                &mut guard_free,
            )?;
        }
        let units: Vec<u16> = line.encode_utf16().collect();
        let mut start = 0;
        while start < units.len() {
            cancel.check()?;
            let mut end = (start + CHUNK).min(units.len());
            // Never split a surrogate pair.
            if end < units.len() && (0xDC00..0xE000).contains(&units[end]) {
                end -= 1;
            }
            let chunk = &units[start..end];
            for down in [true, false] {
                let Some(e) = CGEvent::new_keyboard_event(source().as_deref(), 0, down) else {
                    return err(ErrorCode::Failed, "couldn't make a key event");
                };
                // SAFETY: `chunk` holds `chunk.len()` UTF-16 units.
                unsafe {
                    CGEvent::keyboard_set_unicode_string(Some(&e), chunk.len() as _, chunk.as_ptr())
                };
                post(pid, &e);
            }
            start = end;
        }
    }
    Ok(())
}

struct NoRelease;
impl Release for NoRelease {
    fn release(&self, _: Held) {}
}

/// Releases whatever a request left pressed: a mouse-up to the same window.
pub struct Releaser;

impl Release for Releaser {
    fn release(&self, h: Held) {
        match h {
            Held::Button {
                pid,
                window,
                button,
            } => {
                let b = match button {
                    1 => Button::Right,
                    2 => Button::Middle,
                    _ => Button::Left,
                };
                let (_, up, _) = button_types(b);
                let Some(e) = CGEvent::new_mouse_event(
                    source().as_deref(),
                    up,
                    CGPoint::new(0.0, 0.0),
                    cg_button(b),
                ) else {
                    return;
                };
                CGEvent::set_integer_value_field(
                    Some(&e),
                    CGEventField(FIELD_MOUSE_WINDOW),
                    i64::from(window),
                );
                post(pid, &e);
            }
            Held::Key { pid, keycode } => {
                if let Some(e) = CGEvent::new_keyboard_event(source().as_deref(), keycode, false) {
                    post(pid, &e);
                }
            }
        }
    }
}
