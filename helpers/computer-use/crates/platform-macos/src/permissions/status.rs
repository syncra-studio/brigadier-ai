//! Low-level TCC permission status probes.
//!
//! Mirrors Swift `Permissions.currentStatus()` / `Permissions.requestAccessibility()`
//! / `Permissions.requestScreenRecording()` — bare booleans, no UI.  Used by
//! both the `check_permissions` MCP tool and the startup permissions gate
//! (`super::gate`).

/// Set (to `1`) by the helper's own tests on the processes they start.
/// Every build of such a process is a new unsigned identity, and macOS adds
/// an entry to its privacy lists for each identity that asks about or uses
/// accessibility or screen capture, so a test process must do neither: it
/// reports nothing granted, raises no prompt, and runs only the tools in
/// [`TCC_FREE_TOOLS`].
pub const NO_TCC_FOR_TESTS_ENV: &str = "BRIGADIER_CU_NO_TCC_FOR_TESTS";

/// Tools that need neither accessibility nor screen capture.
pub const TCC_FREE_TOOLS: &[&str] = &[
    "get_screen_size",
    "get_cursor_position",
    "get_config",
    "set_config",
    "list_apps",
    "move_cursor",
    "get_agent_cursor_state",
    "set_agent_cursor_enabled",
    "set_agent_cursor_motion",
    "set_agent_cursor_style",
    // Recording captures only through the screenshot and video paths, which
    // a test process skips; a replay runs each step through this list again.
    "start_recording",
    "stop_recording",
    "get_recording_state",
    "replay_trajectory",
];

/// True in a test process that must make no accessibility or screen-capture
/// call at all (see [`NO_TCC_FOR_TESTS_ENV`]), and always in this crate's own
/// unit tests.
pub fn tcc_off_for_tests() -> bool {
    cfg!(test) || tcc_off_by_env()
}

/// [`tcc_off_for_tests`] for a process started with [`NO_TCC_FOR_TESTS_ENV`]
/// only, so this crate's unit tests can still exercise what the switch
/// changes.
pub fn tcc_off_by_env() -> bool {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| std::env::var(NO_TCC_FOR_TESTS_ENV).is_ok_and(|v| v == "1"))
}

/// Snapshot of which TCC grants are active for the current process.
///
/// Field naming matches the JSON shape used by the `check_permissions`
/// MCP tool: `accessibility` + `screen_recording`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PermissionsStatus {
    pub accessibility: bool,
    #[serde(rename = "screen_recording")]
    pub screen_recording: bool,
}

/// User-facing state of the two-permission onboarding flow.
///
/// Keeping this as an explicit four-state model avoids treating the grant as
/// one boolean. macOS can apply either permission first, and the onboarding UI
/// must immediately advance to the remaining permission in both orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionGrantState {
    NoneGranted,
    AccessibilityGranted,
    ScreenRecordingGranted,
    BothGranted,
}

impl PermissionsStatus {
    /// True when **both** required TCC grants are active.
    pub fn all_granted(self) -> bool {
        self.accessibility && self.screen_recording
    }

    /// Convert the live booleans into the onboarding state machine.
    pub fn grant_state(self) -> PermissionGrantState {
        match (self.accessibility, self.screen_recording) {
            (false, false) => PermissionGrantState::NoneGranted,
            (true, false) => PermissionGrantState::AccessibilityGranted,
            (false, true) => PermissionGrantState::ScreenRecordingGranted,
            (true, true) => PermissionGrantState::BothGranted,
        }
    }
}

/// Read the live TCC status for both required grants.  Cheap to call
/// repeatedly — both probes are quick C-level calls into the system
/// TCC daemon.
///
/// Mirrors Swift `Permissions.currentStatus()`.  Difference: Swift uses
/// `SCShareableContent.excludingDesktopWindows` (ScreenCaptureKit) for
/// the screen recording probe, which is unavailable from Rust without
/// large bindings.  `CGPreflightScreenCaptureAccess` is Apple's
/// documented preflight API for the same grant and is accurate on
/// macOS 11+.
pub fn current_status() -> PermissionsStatus {
    PermissionsStatus {
        accessibility: accessibility_granted(),
        screen_recording: screen_recording_granted(),
    }
}

/// Live AX trust state — `AXIsProcessTrusted()`.
pub fn accessibility_granted() -> bool {
    if tcc_off_for_tests() {
        return false;
    }
    unsafe { crate::ax::bindings::AXIsProcessTrusted() }
}

/// Live Screen Recording grant state — `CGPreflightScreenCaptureAccess()`.
///
/// This is the only probe.  Earlier versions fell back to
/// `!all_windows().is_empty()` when preflight returned false, on the
/// theory that `CGWindowListCopyWindowInfo` returning real windows
/// implied the grant was active.  That theory is wrong:
/// `CGWindowListCopyWindowInfo` returns window IDs and bounds for **any**
/// process without requiring the Screen Recording grant — only window
/// titles are gated.  The fallback therefore returned `true` on any
/// populated desktop regardless of grant state, which (a) made
/// `check_permissions` report a false positive after `tccutil reset
/// ScreenCapture ai.brigadier.computer-use`, and (b) short-circuited the
/// startup permissions gate's prompt for users who had never granted SR.
pub fn screen_recording_granted() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }
    if tcc_off_for_tests() {
        return false;
    }
    unsafe { CGPreflightScreenCaptureAccess() }
}

/// Raise the Accessibility TCC prompt if not yet granted.  No-op when
/// already active.  Mirrors Swift `Permissions.requestAccessibility()`.
pub fn request_accessibility() -> bool {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::string::CFString;

    if tcc_off_for_tests() {
        return false;
    }
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let val = CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), val.as_CFType())]);
    unsafe { crate::ax::bindings::AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef()) }
}

/// Raise the Screen Recording TCC prompt if not yet granted.
/// Mirrors Swift `Permissions.requestScreenRecording()`.
pub fn request_screen_recording() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    if tcc_off_for_tests() {
        return false;
    }
    unsafe { CGRequestScreenCaptureAccess() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_state_preserves_every_partial_state() {
        let state = |accessibility, screen_recording| PermissionsStatus {
            accessibility,
            screen_recording,
        };

        assert_eq!(
            state(false, false).grant_state(),
            PermissionGrantState::NoneGranted
        );
        assert_eq!(
            state(true, false).grant_state(),
            PermissionGrantState::AccessibilityGranted
        );
        assert_eq!(
            state(false, true).grant_state(),
            PermissionGrantState::ScreenRecordingGranted
        );
        assert_eq!(
            state(true, true).grant_state(),
            PermissionGrantState::BothGranted
        );
    }

    #[test]
    fn a_test_process_never_asks_macos_about_permissions() {
        assert!(tcc_off_for_tests());
        assert!(!accessibility_granted());
        assert!(!screen_recording_granted());
        assert!(!request_accessibility());
        assert!(!request_screen_recording());
        assert!(!current_status().accessibility);
        assert!(!current_status().screen_recording);
    }

    #[test]
    fn all_granted_only_accepts_both_grants() {
        for (accessibility, screen_recording, expected) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            assert_eq!(
                PermissionsStatus {
                    accessibility,
                    screen_recording,
                }
                .all_granted(),
                expected
            );
        }
    }
}
