//! macOS lookups for the native target guard
//! ([`cu_core::target_policy`]). The registry runs
//! [`enforce_native`] before every tool call, in every profile, and the MCP
//! proxy runs it again before forwarding, so the guard holds even when the
//! helper daemon on the other end of the socket is an older build.

use cu_core::protocol::ToolResult;
use cu_core::target_policy::{self, TargetIdentity, TargetResolver};
use serde_json::Value;

/// The running app that owns `pid`, from NSRunningApplication (no
/// Accessibility or Screen Recording grant needed). `None` for a process
/// that is not a registered app.
fn app_for_pid(pid: i64) -> Option<TargetIdentity> {
    use objc2_app_kit::NSRunningApplication;
    let pid32 = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
    unsafe {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid32)?;
        let bundle_id = app.bundleIdentifier().map(|value| value.to_string());
        let name = app
            .localizedName()
            .map(|value| value.to_string())
            .or_else(|| bundle_id.clone())
            .unwrap_or_default();
        Some(TargetIdentity {
            name,
            bundle_id,
            pid: Some(pid),
        })
    }
}

/// The process that owns a WindowServer window.
fn pid_for_window(window_id: u64) -> Option<i64> {
    let window_id = u32::try_from(window_id).ok()?;
    crate::windows::all_windows()
        .into_iter()
        .find(|window| window.window_id == window_id)
        .map(|window| i64::from(window.pid))
}

/// The registry's target policy hook on macOS.
pub fn enforce_native(tool: &str, args: &Value, allowed: &[String]) -> Result<(), ToolResult> {
    target_policy::enforce(
        tool,
        args,
        allowed,
        &TargetResolver {
            app_for_pid: &app_for_pid,
            pid_for_window: &pid_for_window,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_driver_process_itself_is_refused() {
        let own = std::process::id();
        let refused = enforce_native("click", &json!({"pid": own}), &[]).unwrap_err();
        assert_eq!(
            refused.structured_content.unwrap()["code"],
            "target_not_allowed"
        );
    }

    #[test]
    fn a_pid_that_is_not_an_app_is_not_refused() {
        // pid 1 (launchd) is not a registered app.
        assert!(enforce_native("get_window_state", &json!({"pid": 1}), &[]).is_ok());
    }
}
