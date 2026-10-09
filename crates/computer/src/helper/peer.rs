//! Who is on the other end of a connection (§4.2): its pid, and in signed builds whether its
//! code signature is from this helper's own team.
//!
//! Security.framework is C; this file is where the helper's unsafe code for it lives, and
//! every block says why it holds.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::ptr::NonNull;

use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};

type OsStatus = i32;
/// `SecCodeRef`, `SecStaticCodeRef` and `SecRequirementRef` are CF objects.
type SecRef = *mut c_void;

const K_SEC_CS_DEFAULT_FLAGS: u32 = 0;
const K_SEC_CS_SIGNING_INFORMATION: u32 = 1 << 1;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecCodeCopySelf(flags: u32, code: *mut SecRef) -> OsStatus;
    fn SecCodeCopySigningInformation(code: SecRef, flags: u32, info: *mut SecRef) -> OsStatus;
    fn SecCodeCopyGuestWithAttributes(
        host: SecRef,
        attributes: *const c_void,
        flags: u32,
        guest: *mut SecRef,
    ) -> OsStatus;
    fn SecRequirementCreateWithString(
        text: *const c_void,
        flags: u32,
        req: *mut SecRef,
    ) -> OsStatus;
    fn SecCodeCheckValidity(code: SecRef, flags: u32, req: SecRef) -> OsStatus;
    static kSecCodeInfoTeamIdentifier: &'static CFString;
    static kSecGuestAttributePid: &'static CFString;
}

/// Takes ownership of a CF object a `Copy`/`Create` call returned.
fn owned(status: OsStatus, ptr: SecRef) -> Option<CFRetained<CFType>> {
    if status != 0 {
        return None;
    }
    // SAFETY: on success the call returned a +1 CF object (or null, which `NonNull` rejects),
    // and `CFRetained` releases it exactly once.
    NonNull::new(ptr.cast::<CFType>()).map(|p| unsafe { CFRetained::from_raw(p) })
}

fn raw(obj: &CFRetained<CFType>) -> SecRef {
    CFRetained::as_ptr(obj).as_ptr().cast()
}

/// The pid of the process on the other end of a local socket.
pub fn peer_pid(stream: &UnixStream) -> std::io::Result<i32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: LOCAL_PEERPID writes one `pid_t` into a buffer of `len` bytes, which `pid` is.
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &mut len,
        )
    };
    if r == 0 {
        Ok(pid)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// This process's signing team, or `None` when it is signed ad hoc or not at all.
pub fn own_team() -> Option<String> {
    let mut code: SecRef = std::ptr::null_mut();
    // SAFETY: the out pointer is valid for one write.
    let code = owned(
        unsafe { SecCodeCopySelf(K_SEC_CS_DEFAULT_FLAGS, &mut code) },
        code,
    )?;
    let mut info: SecRef = std::ptr::null_mut();
    // SAFETY: `code` is a live SecCode, which the call accepts as a static code; the out
    // pointer is valid for one write.
    let info = owned(
        unsafe {
            SecCodeCopySigningInformation(raw(&code), K_SEC_CS_SIGNING_INFORMATION, &mut info)
        },
        info,
    )?;
    // SAFETY: the signing information is a dictionary with string keys.
    let info: CFRetained<CFDictionary<CFString, CFType>> =
        unsafe { CFRetained::cast_unchecked(info) };
    // SAFETY: a static CFString key the framework exports.
    let team = info.get(unsafe { kSecCodeInfoTeamIdentifier })?;
    let team = team.downcast::<CFString>().ok()?.to_string();
    (!team.is_empty()).then_some(team)
}

/// Whether the process `pid` is validly signed by Apple-issued certificates of team `team`.
pub fn same_team(pid: i32, team: &str) -> bool {
    // A team id is ten letters and digits; anything else can't be put in a requirement safely.
    if team.is_empty() || !team.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    // SAFETY: a static CFString key the framework exports.
    let key: &CFString = unsafe { kSecGuestAttributePid };
    let pid = CFNumber::new_i32(pid);
    let attrs = CFDictionary::<CFString, CFNumber>::from_slices(&[key], &[&pid]);
    let mut guest: SecRef = std::ptr::null_mut();
    // SAFETY: a null host asks the system for the guest; the attributes map the documented pid
    // key to a CFNumber; the out pointer is valid for one write.
    let guest = owned(
        unsafe {
            SecCodeCopyGuestWithAttributes(
                std::ptr::null_mut(),
                CFRetained::as_ptr(&attrs).as_ptr().cast(),
                K_SEC_CS_DEFAULT_FLAGS,
                &mut guest,
            )
        },
        guest,
    );
    let Some(guest) = guest else { return false };
    let text = CFString::from_str(&format!(
        "anchor apple generic and certificate leaf[subject.OU] = \"{team}\""
    ));
    let mut req: SecRef = std::ptr::null_mut();
    // SAFETY: `text` is a live CFString; the out pointer is valid for one write.
    let req = owned(
        unsafe {
            SecRequirementCreateWithString(
                CFRetained::as_ptr(&text).as_ptr().cast(),
                K_SEC_CS_DEFAULT_FLAGS,
                &mut req,
            )
        },
        req,
    );
    let Some(req) = req else { return false };
    // SAFETY: both are live objects of the types the call takes.
    unsafe { SecCodeCheckValidity(raw(&guest), K_SEC_CS_DEFAULT_FLAGS, raw(&req)) == 0 }
}

/// Refuses a peer that isn't the parent daemon (when one was named) or, when this helper is
/// signed by a team, isn't signed by the same team. An ad-hoc or unsigned helper (every local
/// and development build) has no team to compare, so only the pid and the token protect it.
pub fn check(stream: &UnixStream, parent: Option<i32>, team: Option<&str>) -> Result<(), String> {
    if parent.is_none() && team.is_none() {
        return Ok(());
    }
    let pid = peer_pid(stream).map_err(|e| format!("can't tell who connected: {e}"))?;
    if let Some(parent) = parent
        && pid != parent
    {
        return Err(format!(
            "pid {pid} isn't the daemon that started the helper"
        ));
    }
    if let Some(team) = team
        && !same_team(pid, team)
    {
        return Err(format!("pid {pid} isn't signed by team {team}"));
    }
    Ok(())
}
