//! Makes one window of a background app its key window, without bringing the
//! app to the front.
//!
//! Chromium and Electron insert typed text only while their window is
//! AppKit's key window. The window server's paired make-key event records,
//! posted straight to the target process, give that window key status inside
//! its app while the app stays in the background: no front-process switch,
//! no raise, no Space change, and the user's keys keep going to the app they
//! work in. Keys posted to the pid afterwards then reach the page.

use std::ffi::{c_char, c_void};
use std::sync::OnceLock;
use std::time::Duration;

/// How long the target needs to take the key window before keys are sent.
/// Keys sent at once were dropped about one time in ten.
pub const SETTLE: Duration = Duration::from_millis(10);

/// `OSStatus SLPSPostEventRecordTo(const ProcessSerialNumber *, const uint8_t *)`.
type PostEventRecordTo = unsafe extern "C" fn(*const c_void, *const u8) -> i32;

fn post_event_record_to() -> Option<PostEventRecordTo> {
    static SYM: OnceLock<Option<usize>> = OnceLock::new();
    let ptr = *SYM.get_or_init(|| {
        // SAFETY: a symbol lookup in the already loaded images.
        let ptr = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"SLPSPostEventRecordTo".as_ptr() as *const c_char,
            )
        };
        (!ptr.is_null()).then_some(ptr as usize)
    });
    // SAFETY: the symbol has this signature on every macOS that ships it.
    ptr.map(|ptr| unsafe { std::mem::transmute::<usize, PostEventRecordTo>(ptr) })
}

/// The 248-byte make-key event record for `window_id`; `kind` 1 is the
/// press half and 2 the release half.
fn make_key_record(window_id: u32, kind: u8) -> [u8; 0xF8] {
    let mut record = [0u8; 0xF8];
    record[0x04] = 0xF8;
    record[0x08] = kind;
    record[0x3A] = 0x10;
    record[0x3C..0x40].copy_from_slice(&window_id.to_le_bytes());
    record[0x20..0x30].fill(0xFF);
    record
}

/// Makes `window_id` the key window of its app `pid` without activating the
/// app. `false` when the window's process can't be found or the window
/// server refused a record; nothing else is tried then.
pub fn make_key_in_background(pid: i32, window_id: u32) -> bool {
    let mut psn = [0u8; 8];
    // Also loads the window server's private framework the symbol lives in.
    if !platform_macos::input::skylight::get_process_psn_for_window(window_id, pid, &mut psn) {
        return false;
    }
    let Some(post) = post_event_record_to() else {
        return false;
    };
    for kind in [0x01, 0x02] {
        let record = make_key_record(window_id, kind);
        // SAFETY: both buffers outlive the call; the record is 248 bytes.
        if unsafe { post(psn.as_ptr() as *const c_void, record.as_ptr()) } != 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_names_the_window_and_the_half() {
        let press = make_key_record(0x7856_3412, 0x01);
        let release = make_key_record(0x7856_3412, 0x02);
        assert_eq!(press.len(), 0xF8);
        assert_eq!(press[0x04], 0xF8);
        assert_eq!(press[0x08], 0x01);
        assert_eq!(release[0x08], 0x02);
        assert_eq!(press[0x3A], 0x10);
        assert_eq!(&press[0x3C..0x40], &[0x12, 0x34, 0x56, 0x78]);
        assert!(press[0x20..0x30].iter().all(|b| *b == 0xFF));
        assert_eq!(press[..0x08].iter().filter(|b| **b != 0).count(), 1);
    }
}
