//! Every private call and undocumented value the macOS backend uses, and nothing else
//! (§12 of docs/COMPUTER-USE-PLAN.md). Symbols are resolved at run time; a missing one turns
//! off only the capability that needs it.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;

use objc2_application_services::AXUIElement;
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::CGEvent;

/// A Process Manager serial number.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Psn {
    pub high: u32,
    pub low: u32,
}

/// The event field AppKit reads for a pid-posted mouse down, up or drag's window.
pub const FIELD_MOUSE_WINDOW: u32 = 51;
/// The same for a mouse-moved event.
pub const FIELD_MOVED_WINDOW: u32 = 103;

type PostRecord = unsafe extern "C" fn(*const Psn, *const u8) -> i32;
type GetFront = unsafe extern "C" fn(*mut Psn) -> i32;
type SetWindowLocation = unsafe extern "C" fn(*const c_void, CGPoint);
type AxGetWindow = unsafe extern "C" fn(*const c_void, *mut u32) -> i32;
type GetProcessForPid = unsafe extern "C" fn(libc::pid_t, *mut Psn) -> i32;

pub struct Private {
    post_record: Option<PostRecord>,
    get_front: Option<GetFront>,
    set_window_location: Option<SetWindowLocation>,
    ax_get_window: Option<AxGetWindow>,
    get_process_for_pid: Option<GetProcessForPid>,
}

const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";

fn sym(handle: *mut c_void, name: &CStr) -> *mut c_void {
    // SAFETY: dlsym only reads the handle and the NUL-terminated name.
    unsafe { libc::dlsym(handle, name.as_ptr()) }
}

/// Casts a symbol to its function type, or None when it is missing.
macro_rules! bind {
    ($handle:expr, $name:literal, $ty:ty) => {{
        let p = sym($handle, $name);
        // SAFETY: the symbol has the ABI recorded in the plan's §12 table, validated on
        // macOS 27; a null pointer stays None.
        (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, $ty>(p) })
    }};
}

impl Private {
    pub fn get() -> &'static Private {
        static P: OnceLock<Private> = OnceLock::new();
        P.get_or_init(|| {
            // SAFETY: dlopen of a system framework path; a null handle disables its symbols.
            let sky = unsafe { libc::dlopen(SKYLIGHT.as_ptr(), libc::RTLD_NOW) };
            let any = libc::RTLD_DEFAULT;
            let from_sky = |p: *mut c_void| {
                if sky.is_null() {
                    std::ptr::null_mut()
                } else {
                    p
                }
            };
            Private {
                post_record: {
                    let p = from_sky(sym(sky, c"SLPSPostEventRecordTo"));
                    // SAFETY: as in `bind!`.
                    (!p.is_null())
                        .then(|| unsafe { std::mem::transmute::<*mut c_void, PostRecord>(p) })
                },
                get_front: {
                    let p = from_sky(sym(sky, c"_SLPSGetFrontProcess"));
                    // SAFETY: as in `bind!`.
                    (!p.is_null())
                        .then(|| unsafe { std::mem::transmute::<*mut c_void, GetFront>(p) })
                },
                set_window_location: {
                    let p = from_sky(sym(sky, c"CGEventSetWindowLocation"));
                    // SAFETY: as in `bind!`.
                    (!p.is_null()).then(|| unsafe {
                        std::mem::transmute::<*mut c_void, SetWindowLocation>(p)
                    })
                },
                ax_get_window: bind!(any, c"_AXUIElementGetWindow", AxGetWindow),
                get_process_for_pid: bind!(any, c"GetProcessForPID", GetProcessForPid),
            }
        })
    }

    /// Background clicks need the window location; activation needs the record call.
    pub fn has_pointer(&self) -> bool {
        self.set_window_location.is_some()
    }

    pub fn has_activation(&self) -> bool {
        self.post_record.is_some() && self.get_process_for_pid.is_some()
    }

    /// The window-server id of an accessibility window.
    pub fn ax_window_id(&self, el: &AXUIElement) -> Option<u32> {
        let f = self.ax_get_window?;
        let mut id = 0u32;
        let el: *const AXUIElement = el;
        // SAFETY: `el` is a live AXUIElementRef; `id` is a valid out pointer.
        (unsafe { f(el.cast(), &mut id) } == 0 && id != 0).then_some(id)
    }

    /// Sets a mouse or scroll event's location inside its target window (top left of the frame).
    pub fn set_window_location(&self, event: &CGEvent, p: CGPoint) -> bool {
        let Some(f) = self.set_window_location else {
            return false;
        };
        let event: *const CGEvent = event;
        // SAFETY: `event` is a live CGEventRef.
        unsafe { f(event.cast(), p) };
        true
    }

    /// The window server's front process.
    pub fn server_front(&self) -> Option<Psn> {
        let f = self.get_front?;
        let mut psn = Psn::default();
        // SAFETY: `psn` is a valid out pointer.
        (unsafe { f(&mut psn) } == 0).then_some(psn)
    }

    pub fn psn(&self, pid: i32) -> Option<Psn> {
        let f = self.get_process_for_pid?;
        let mut psn = Psn::default();
        // SAFETY: `psn` is a valid out pointer.
        (unsafe { f(pid, &mut psn) } == 0).then_some(psn)
    }

    fn post(&self, psn: &Psn, record: &[u8; 0xf8]) -> bool {
        let Some(f) = self.post_record else {
            return false;
        };
        // SAFETY: the record is the 0xf8-byte buffer the call reads; `psn` is valid.
        unsafe { f(psn, record.as_ptr()) == 0 }
    }

    /// The focus record: the app believes it is active (`on`) or stops believing it.
    pub fn focus_record(&self, psn: &Psn, window: u32, on: bool) -> bool {
        let mut r = [0u8; 0xf8];
        r[0x04] = 0xf8;
        r[0x08] = 0x0d;
        r[0x3c..0x40].copy_from_slice(&window.to_le_bytes());
        r[0x8a] = if on { 0x01 } else { 0x02 };
        self.post(psn, &r)
    }

    /// The make-key pair: the window becomes key inside its app.
    pub fn make_key(&self, psn: &Psn, window: u32) -> bool {
        let mut ok = true;
        for kind in [0x01u8, 0x02] {
            let mut r = [0u8; 0xf8];
            r[0x04] = 0xf8;
            r[0x08] = kind;
            r[0x20..0x30].fill(0xff);
            r[0x3a] = 0x10;
            r[0x3c..0x40].copy_from_slice(&window.to_le_bytes());
            ok &= self.post(psn, &r);
        }
        ok
    }
}
