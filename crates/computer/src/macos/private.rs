//! Every private call and undocumented value the macOS backend uses, and nothing else
//! (§12 of docs/COMPUTER-USE-PLAN.md). Symbols are resolved at run time; a missing one turns
//! off only the capability that needs it.

use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::sync::OnceLock;

use objc2_application_services::AXUIElement;
use objc2_core_foundation::{CFArray, CFData, CFRetained, CGPoint};
use objc2_core_graphics::{CGEvent, CGImage};

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
type MainConnection = unsafe extern "C" fn() -> i32;
type HwCapture = unsafe extern "C" fn(i32, *const u32, i32, u32) -> *const c_void;
type AxCreateRemote = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type GetProcessForPid = unsafe extern "C" fn(libc::pid_t, *mut Psn) -> i32;
type SetFront = unsafe extern "C" fn(*const Psn, u32, u32) -> i32;
type GetProcessPid = unsafe extern "C" fn(*const Psn, *mut libc::pid_t) -> i32;

/// `_SLPSSetFrontProcessWithOptions`'s mode for a front change the user asked for.
const FRONT_USER_GENERATED: u32 = 0x200;
/// `SLSHWCaptureWindowList`'s options: ignore the global clip shape, nominal and best resolution.
const HW_CAPTURE_OPTIONS: u32 = 1 << 11 | 1 << 9 | 1 << 8;

pub struct Private {
    post_record: Option<PostRecord>,
    get_front: Option<GetFront>,
    set_window_location: Option<SetWindowLocation>,
    ax_get_window: Option<AxGetWindow>,
    ax_create_remote: Option<AxCreateRemote>,
    main_connection: Option<MainConnection>,
    hw_capture: Option<HwCapture>,
    get_process_for_pid: Option<GetProcessForPid>,
    set_front: Option<SetFront>,
    get_process_pid: Option<GetProcessPid>,
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
                main_connection: {
                    let p = from_sky(sym(sky, c"SLSMainConnectionID"));
                    // SAFETY: as in `bind!`.
                    (!p.is_null())
                        .then(|| unsafe { std::mem::transmute::<*mut c_void, MainConnection>(p) })
                },
                hw_capture: {
                    let p = from_sky(sym(sky, c"SLSHWCaptureWindowList"));
                    // SAFETY: as in `bind!`.
                    (!p.is_null())
                        .then(|| unsafe { std::mem::transmute::<*mut c_void, HwCapture>(p) })
                },
                set_front: {
                    let p = from_sky(sym(sky, c"_SLPSSetFrontProcessWithOptions"));
                    // SAFETY: as in `bind!`.
                    (!p.is_null())
                        .then(|| unsafe { std::mem::transmute::<*mut c_void, SetFront>(p) })
                },
                ax_get_window: bind!(any, c"_AXUIElementGetWindow", AxGetWindow),
                ax_create_remote: bind!(any, c"_AXUIElementCreateWithRemoteToken", AxCreateRemote),
                get_process_pid: bind!(any, c"GetProcessPID", GetProcessPid),
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

    /// The accessibility element `id` of `pid`, made from its remote token: the pid (i32), 0,
    /// the tag `coco` (i32), then the element id (u64). It reaches a window on another Space,
    /// which the app's window list leaves out.
    pub fn ax_remote_element(&self, pid: i32, id: u64) -> Option<CFRetained<AXUIElement>> {
        let f = self.ax_create_remote?;
        let mut token = [0u8; 20];
        token[0..4].copy_from_slice(&pid.to_ne_bytes());
        token[8..12].copy_from_slice(&0x636f_636f_i32.to_ne_bytes());
        token[12..20].copy_from_slice(&id.to_ne_bytes());
        let data = CFData::from_bytes(&token);
        // SAFETY: `data` is a live CFDataRef for the call; a non-null result is +1 retained
        // (a Create call).
        let p = unsafe { f(CFRetained::as_ptr(&data).as_ptr().cast_const().cast()) };
        NonNull::new(p.cast::<AXUIElement>()).map(|p| unsafe { CFRetained::from_raw(p) })
    }

    /// The window's backing store at its full resolution, from the window server: it has one
    /// for a window on another Space, which ScreenCaptureKit waits on for a frame that never
    /// comes (measured 2026-10-09: about 100 ms for a 900×632 pt window at 2×).
    pub fn capture_window(&self, window: u32) -> Option<CFRetained<CGImage>> {
        let (conn, f) = (self.main_connection?, self.hw_capture?);
        // SAFETY: one valid window id; a non-null result is a +1 retained array of images.
        let p = unsafe { f(conn(), &window, 1, HW_CAPTURE_OPTIONS) };
        let arr = NonNull::new(p.cast_mut().cast::<CFArray>())
            .map(|p| unsafe { CFRetained::from_raw(p) })?;
        // SAFETY: the array holds CGImages.
        let arr: CFRetained<CFArray<CGImage>> = unsafe { CFRetained::cast_unchecked(arr) };
        arr.iter().next()
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

    /// Brings a process to the front through the window server, with `window` (0: the app's
    /// own choice) as its key window: the foreground rung's raise and its give-back. The
    /// system's activation calls are declined from a background process.
    pub fn set_front(&self, pid: i32, window: u32) -> bool {
        let (Some(f), Some(psn)) = (self.set_front, self.psn(pid)) else {
            return false;
        };
        // SAFETY: `psn` is a valid serial number read just now.
        unsafe { f(&psn, window, FRONT_USER_GENERATED) == 0 }
    }

    /// The pid of the window server's front process. Fresh on any thread, where AppKit's
    /// frontmost app updates only on a running main run loop.
    pub fn front_pid(&self) -> Option<i32> {
        let f = self.get_process_pid?;
        let psn = self.server_front()?;
        let mut pid: libc::pid_t = 0;
        // SAFETY: `psn` was just read; `pid` is a valid out pointer.
        (unsafe { f(&psn, &mut pid) } == 0 && pid > 0).then_some(pid)
    }

    /// A running process's serial number, tried again on a failure: one background click in
    /// 1,600 of a full bench (2026-10-10) ended in `unsupported_capability`, which during a
    /// click only this lookup gives, while its process kept running.
    pub fn psn(&self, pid: i32) -> Option<Psn> {
        let f = self.get_process_for_pid?;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let mut psn = Psn::default();
            // SAFETY: `psn` is a valid out pointer.
            if unsafe { f(pid, &mut psn) } == 0 {
                return Some(psn);
            }
        }
        None
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
