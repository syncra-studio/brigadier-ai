//! Background input synthesis for Windows.
//!
//! Strategy:
//! - Mouse: PostMessageW(WM_LBUTTONDOWN/UP) with packed LPARAM(x,y).
//!   PostMessage is async and does NOT activate the target window.
//! - Keyboard text: PostMessageW(WM_CHAR) per character.
//! - Key press: PostMessageW(WM_KEYDOWN) + PostMessageW(WM_KEYUP).
//!
//! For clicks, coordinates are window-client-relative (ScreenToClient applied
//! if screen coords are given). We pack with MAKELPARAM(x,y).

pub mod delivery;
pub mod inject;
pub mod keyboard;
pub mod mouse;
pub mod text_policy;

pub(crate) use inject::{force_foreground_assisted, force_foreground_attached};
pub use inject::{
    inject_click_screen, point_in_window_bounds, point_on_virtual_desktop, window_is_iconic,
    ForegroundLockGuard, NoActivateGuard,
};
pub use keyboard::{
    is_xaml_host_hwnd, post_char, post_key, post_type_text, post_type_text_with_delay,
    send_key_synthesized, send_key_synthesized_after_focus, send_text_synthesized,
    send_text_synthesized_after_focus, wait_for_focused_descendant,
};
pub use mouse::{
    has_chromium_descendant, is_chromium_target_window, post_click, post_click_screen,
    send_click_synthesized, send_click_synthesized_active_mods, send_click_synthesized_mods,
    send_wheel_synthesized,
};

use cua_driver_core::window_state_view::windows_integrity_message as integrity_block_message;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

/// Windows mandatory integrity-level RIDs. Higher = more privileged. See
/// https://learn.microsoft.com/en-us/windows/win32/secauthz/mandatory-integrity-control
#[allow(dead_code)]
mod il {
    pub const UNTRUSTED: u32 = 0x0000;
    pub const LOW: u32 = 0x1000;
    pub const MEDIUM: u32 = 0x2000;
    pub const MEDIUM_PLUS: u32 = 0x2100;
    pub const HIGH: u32 = 0x3000;
    pub const SYSTEM: u32 = 0x4000;
}

/// Read the mandatory integrity level (the last sub-authority of the
/// integrity SID) of a process handle. Returns `None` on any API failure.
unsafe fn process_integrity_rid(process: HANDLE) -> Option<u32> {
    let mut token = HANDLE::default();
    if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
        return None;
    }
    let mut needed: u32 = 0;
    // First call: probe required buffer size. Returns ERROR_INSUFFICIENT_BUFFER
    // and writes `needed`; we don't check the error because we always need a
    // second call to actually populate `buf`.
    let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut needed);
    if needed == 0 {
        let _ = CloseHandle(token);
        return None;
    }
    let mut buf = vec![0u8; needed as usize];
    let ok = GetTokenInformation(
        token,
        TokenIntegrityLevel,
        Some(buf.as_mut_ptr() as _),
        needed,
        &mut needed,
    )
    .is_ok();
    let _ = CloseHandle(token);
    if !ok {
        return None;
    }
    let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
    let sid = tml.Label.Sid;
    let count_ptr = GetSidSubAuthorityCount(sid);
    if count_ptr.is_null() {
        return None;
    }
    let count = *count_ptr;
    if count == 0 {
        return None;
    }
    let rid_ptr = GetSidSubAuthority(sid, (count - 1) as u32);
    if rid_ptr.is_null() {
        return None;
    }
    Some(*rid_ptr)
}

/// If posting messages from the current process to `hwnd` would be silently
/// blocked by UIPI (User Interface Privilege Isolation), return a diagnostic
/// string the caller should surface as an actionable error. Otherwise `None`.
///
/// UIPI blocks `PostMessage` / `SendMessage` of input-class messages
/// (`WM_KEYDOWN`, `WM_KEYUP`, `WM_CHAR`, `WM_LBUTTONDOWN`, etc.) from a
/// lower-integrity process to a higher-integrity window. Crucially, the
/// `PostMessage` call still returns `TRUE` — the message is queued but the
/// elevated target's message pump filters it out before delivery. The lower-
/// integrity sender has no way to detect this from the return value, so
/// without an explicit check upstream, `type_text` / `hotkey` / `click`
/// silently no-op against elevated apps.
///
/// Returning a diagnostic string before the `PostMessage` call is the
/// minimum honest behavior: the caller learns it can't drive this target
/// and why. Long-term fix is to route through `SendInput` (system input
/// queue, UIPI-permitted) from the UIAccess'd worker — out of scope here.
pub fn post_message_blocked_by_uipi(hwnd: u64) -> Option<String> {
    let h = HWND(hwnd as *mut _);
    let mut pid: u32 = 0;
    if unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) } == 0 || pid == 0 {
        return None;
    }
    let target_handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let message = process_control_integrity_refusal(target_handle).ok().flatten();
    let _ = unsafe { CloseHandle(target_handle) };
    message

}

/// Check the exact process handle used by kill_app, including targets with no
/// windows. Fail closed if either token cannot be inspected.
pub(crate) fn process_control_integrity_refusal(target: HANDLE) -> Result<Option<String>, String> {
    let own = unsafe { process_integrity_rid(GetCurrentProcess()) }
        .ok_or_else(|| "Windows could not inspect this helper's privileges. No termination was attempted.".to_owned())?;
    let target = unsafe { process_integrity_rid(target) }
        .ok_or_else(|| "Windows could not inspect the target process's privileges. No termination was attempted.".to_owned())?;
    Ok(integrity_block_message(own, target).map(str::to_owned))
}

#[cfg(test)]
mod integrity_tests {
    use super::*;

    #[test]
    fn administrator_windows_explain_the_read_and_input_limit_plainly() {
        let message = integrity_block_message(il::MEDIUM, il::HIGH).unwrap();
        assert!(message.starts_with("This window runs as administrator"));
        assert!(message.contains("cannot be read or controlled"));
        assert!(message.contains("restart the computer-use helper as administrator"));
        assert!(integrity_block_message(il::HIGH, il::HIGH).is_none());
        assert!(integrity_block_message(il::HIGH, il::MEDIUM).is_none());
        assert!(integrity_block_message(il::MEDIUM, il::MEDIUM).is_none());
    }

    #[test]
    fn lower_privilege_helpers_do_not_mislabel_normal_apps_as_administrator() {
        let message = integrity_block_message(il::LOW, il::MEDIUM).unwrap();
        assert!(message.contains("higher privileges"));
        assert!(!message.contains("administrator"));
    }
}
