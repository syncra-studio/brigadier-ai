//! macOS `health_report` provider.
//!
//! Implements [`HealthCheckProvider`] for darwin. Reuses the existing
//! TCC plumbing in `crate::permissions::status` rather than
//! re-implementing it — the goal of the tool is one stable diagnostic
//! call, not duplicate health logic.
//!
//! Schema contract + filter / rollup logic lives in
//! `cu_core::health_report`. This file owns the platform
//! probes only.

use async_trait::async_trait;
use cu_core::health_report::{
    CheckData, CheckEntry, HealthCheckProvider, NAME_AX_CAPABILITY, NAME_BINARY_VERSION,
    NAME_BUNDLE_IDENTITY, NAME_PLATFORM_SUPPORTED, NAME_SCREEN_CAPTURE_CAPABILITY,
    NAME_SESSION_ACTIVE, NAME_TCC_ACCESSIBILITY, NAME_TCC_SCREEN_RECORDING,
};

use crate::permissions::status::{accessibility_granted, current_status, screen_recording_granted};

/// macOS run order, in the order consumers see them reflected back in
/// `report.checks[]`. Matches the Swift PR #1905 contract.
pub const MACOS_CHECK_NAMES: &[&str] = &[
    NAME_BINARY_VERSION,
    NAME_PLATFORM_SUPPORTED,
    NAME_SESSION_ACTIVE,
    NAME_BUNDLE_IDENTITY,
    NAME_TCC_ACCESSIBILITY,
    NAME_TCC_SCREEN_RECORDING,
    NAME_AX_CAPABILITY,
    NAME_SCREEN_CAPTURE_CAPABILITY,
];

/// The canonical bundle identifier whose TCC grants matter for the
/// daemon. The `bundle_identity` check passes when the running process
/// reports this id.
pub const CANONICAL_BUNDLE_ID: &str = "ai.brigadier.computer-use";

pub struct MacosHealthProvider;

#[async_trait]
impl HealthCheckProvider for MacosHealthProvider {
    fn platform(&self) -> &'static str {
        "darwin"
    }

    fn check_names(&self) -> &'static [&'static str] {
        MACOS_CHECK_NAMES
    }

    async fn run_check(&self, name: &str) -> CheckEntry {
        match name {
            NAME_BINARY_VERSION => check_binary_version(),
            NAME_PLATFORM_SUPPORTED => check_platform_supported(),
            NAME_SESSION_ACTIVE => check_session_active(),
            NAME_BUNDLE_IDENTITY => check_bundle_identity(),
            NAME_TCC_ACCESSIBILITY => check_tcc_accessibility(),
            NAME_TCC_SCREEN_RECORDING => check_tcc_screen_recording(),
            NAME_AX_CAPABILITY => check_ax_capability(),
            NAME_SCREEN_CAPTURE_CAPABILITY => check_screen_capture_capability().await,
            // Defensive: the dispatcher only forwards names in
            // `check_names()`, so this branch should be unreachable.
            other => CheckEntry::skip(
                other.to_owned(),
                "Unknown check name (not implemented on this platform).",
            ),
        }
    }
}

// ── Individual checks ────────────────────────────────────────────────────────

pub(crate) fn check_binary_version() -> CheckEntry {
    // CARGO_PKG_VERSION is a compiled-in constant; reaching this code
    // already implies the binary built. Always pass.
    CheckEntry::pass(
        NAME_BINARY_VERSION,
        format!("brigadier-cu {}", env!("CARGO_PKG_VERSION")),
    )
}

pub(crate) fn check_platform_supported() -> CheckEntry {
    // Reaching this code path on macOS implies a supported platform —
    // the macOS build is gated behind `cfg(target_os = "macos")`.
    let os_version = sw_vers_product_version().unwrap_or_else(|| "unknown".to_owned());
    let arch = arch_label();
    CheckEntry::pass(
        NAME_PLATFORM_SUPPORTED,
        format!("macOS {os_version} ({arch})"),
    )
    .with_data(CheckData {
        os_version: Some(os_version),
        architecture: Some(arch.to_owned()),
        ..Default::default()
    })
}

pub(crate) fn check_session_active() -> CheckEntry {
    // We are servicing this MCP call, so by construction the session
    // is up. The check exists so consumers can hard-code a canonical
    // "is the server reachable?" signal in a fixed shape.
    CheckEntry::pass(NAME_SESSION_ACTIVE, "MCP session is active.")
}

pub(crate) fn check_bundle_identity() -> CheckEntry {
    let bid = current_bundle_identifier();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_default();

    check_bundle_identity_for_context(bid, exe, external_permission_flow_enabled())
}

pub(crate) fn external_permission_flow_enabled() -> bool {
    let from_environment = std::env::var("BRIGADIER_CU_EXTERNAL_PERMISSION_FLOW")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false);
    let brigadier_branded_executable = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_owned()))
        .and_then(|name| name.to_str().map(str::to_owned))
        .as_deref()
        == Some("brigadier-cu");
    from_environment || brigadier_branded_executable
}

fn check_bundle_identity_for_context(
    bid: Option<String>,
    exe: String,
    _external_permission_flow: bool,
) -> CheckEntry {
    let is_canonical = bid.as_deref() == Some(CANONICAL_BUNDLE_ID);
    let is_external_helper = _external_permission_flow
        && exe.contains(".app/Contents/MacOS/")
        && bid
            .as_deref()
            .is_some_and(|identifier| !identifier.is_empty());
    let data = CheckData {
        bundle_identifier: bid.clone(),
        executable_path: if exe.is_empty() { None } else { Some(exe) },
        ..Default::default()
    };
    if is_canonical {
        return CheckEntry::pass(
            NAME_BUNDLE_IDENTITY,
            format!("Bundle is {CANONICAL_BUNDLE_ID}."),
        )
        .with_data(data);
    }
    if is_external_helper {
        let identifier = bid.as_deref().unwrap_or_default();
        return CheckEntry::pass(
            NAME_BUNDLE_IDENTITY,
            format!("Bundle is {identifier}; the embedding host owns permission setup."),
        )
        .with_data(data);
    }
    let (message, hint) = if _external_permission_flow {
        match bid.as_deref() {
            None | Some("") => (
                "Process has no CFBundleIdentifier.".to_owned(),
                "Restart the embedding host so it can launch its bundled Computer Use helper. Do not run the standalone brigadier-cu permission flow."
                    .to_owned(),
            ),
            Some(other) => (
                format!("Bundle is {other}, but the executable is not inside its helper app."),
                "Restart the embedding host's Computer Use runtime so permission status comes from its own LaunchServices helper. Do not run `brigadier-cu permissions grant`."
                    .to_owned(),
            ),
        }
    } else {
        match bid.as_deref() {
            None | Some("") => (
                "Process has no CFBundleIdentifier.".to_owned(),
                format!(
                    "Run the binary inside Brigadier Computer Use.app so TCC grants attribute correctly. \
                 Start the daemon with `open -n -g -a 'Brigadier Computer Use' --args serve` and \
                 connect via `brigadier-cu mcp`."
                ),
            ),
            Some(other) => (
                format!("Bundle is {other}, not {CANONICAL_BUNDLE_ID}."),
                format!(
                    "TCC grants will be attributed to {other}, not the brigadier-cu daemon. \
                 Run via `brigadier-cu mcp` (auto-relaunches inside Brigadier Computer Use.app) or \
                start the daemon manually: `open -n -g -a 'Brigadier Computer Use' --args serve`."
                ),
            ),
        }
    };
    CheckEntry::fail(NAME_BUNDLE_IDENTITY, message, hint).with_data(data)
}

fn check_tcc_accessibility() -> CheckEntry {
    // Reuse the shared probe — do not duplicate AXIsProcessTrusted.
    let status = current_status();
    let data = CheckData {
        bundle_identifier: current_bundle_identifier(),
        ..Default::default()
    };
    if status.accessibility {
        return CheckEntry::pass(NAME_TCC_ACCESSIBILITY, "Accessibility is granted.")
            .with_data(data);
    }
    let hint = if external_permission_flow_enabled() {
        "Grant Accessibility to the Computer Use helper reported by bundle_identity using the embedding host's onboarding. Do not run the standalone brigadier-cu permission flow."
    } else {
        "Grant Accessibility to Brigadier Computer Use.app in System Settings → Privacy & Security → Accessibility. If the process bundle is not ai.brigadier.computer-use (see bundle_identity), the grant must target the responsible app — restart via `brigadier-cu mcp` to relaunch inside Brigadier Computer Use.app."
    };
    CheckEntry::fail(
        NAME_TCC_ACCESSIBILITY,
        "Accessibility is NOT granted for this process.",
        hint,
    )
    .with_data(data)
}

fn check_tcc_screen_recording() -> CheckEntry {
    let granted = screen_recording_granted();
    let data = CheckData {
        bundle_identifier: current_bundle_identifier(),
        ..Default::default()
    };
    if granted {
        return CheckEntry::pass(NAME_TCC_SCREEN_RECORDING, "Screen Recording is granted.")
            .with_data(data);
    }
    let hint = if external_permission_flow_enabled() {
        "Grant Screen Recording to the Computer Use helper reported by bundle_identity using the embedding host's onboarding. Do not run the standalone brigadier-cu permission flow."
    } else {
        "Grant Screen Recording to Brigadier Computer Use.app in System Settings → Privacy & Security → Screen Recording. The grant is attributed to the responsible process — see bundle_identity to confirm the right binary is being prompted."
    };
    CheckEntry::fail(
        NAME_TCC_SCREEN_RECORDING,
        "Screen Recording is NOT granted for this process.",
        hint,
    )
    .with_data(data)
}

fn check_ax_capability() -> CheckEntry {
    // The AX trust gate is the same kAXIsProcessTrusted probe
    // `tcc_accessibility` runs, but ax_capability is the consumer-
    // facing "can I actually drive UI" signal. We separate them so a
    // future change (e.g. capability degraded without TCC denial)
    // doesn't break either contract.
    if accessibility_granted() {
        return CheckEntry::pass(NAME_AX_CAPABILITY, "AX is trusted and reachable.");
    }
    CheckEntry::fail(
        NAME_AX_CAPABILITY,
        "AX is not trusted; UI inspection and event posting will fail.",
        "Resolve tcc_accessibility first — AX capability follows directly from the \
         Accessibility TCC grant.",
    )
}

async fn check_screen_capture_capability() -> CheckEntry {
    // Live ScreenCaptureKit probe — same shape as `check_permissions`'s
    // `screen_recording_capturable`, but answer the consumer-facing
    // capability question and surface the display count.
    //
    // `SCShareableContent::get()` is a synchronous C call; wrap it in
    // spawn_blocking so the async runtime isn't held by a system call
    // that can take 10-100ms on first invocation.
    let result = tokio::task::spawn_blocking(probe_shareable_displays)
        .await
        .unwrap_or(Err("probe task panicked".to_owned()));
    match result {
        Ok(count) => CheckEntry::pass(
            NAME_SCREEN_CAPTURE_CAPABILITY,
            format!("ScreenCaptureKit reachable; {count} display(s) shareable."),
        )
        .with_data(CheckData {
            display_count: Some(count),
            ..Default::default()
        }),
        Err(detail) => CheckEntry::fail(
            NAME_SCREEN_CAPTURE_CAPABILITY,
            "ScreenCaptureKit probe failed.",
            "Confirm tcc_screen_recording is granted. If it is, this is an SCK regression — \
             set capture_mode to `ax` to skip screen capture entirely.",
        )
        .with_data(CheckData {
            error_detail: Some(detail),
            ..Default::default()
        }),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Live ScreenCaptureKit probe — enumerates shareable displays. Writes
/// nothing to disk; we never start a stream. Returns display count on
/// success, error string on failure.
fn probe_shareable_displays() -> Result<u32, String> {
    use crate::capture_deadline::{self, CaptureOperation};
    use screencapturekit::prelude::SCShareableContent;
    capture_deadline::run(
        CaptureOperation::ShareableContent,
        capture_deadline::SHAREABLE_CONTENT_BUDGET,
        || {
            let content = SCShareableContent::get()
                .map_err(|e| anyhow::anyhow!("SCShareableContent::get: {e:?}"))?;
            Ok(content.displays().len() as u32)
        },
    )
    .map_err(|e| format!("{e:#}"))
}

/// Read the running process's `CFBundleIdentifier` via CoreFoundation.
/// Returns `None` when the process has no associated bundle (e.g.
/// running the bare binary outside `Brigadier Computer Use.app`).
pub(crate) fn current_bundle_identifier() -> Option<String> {
    use core_foundation::base::TCFType;
    use core_foundation::bundle::{CFBundle, CFBundleGetMainBundle};
    use core_foundation::string::CFString;

    unsafe {
        let raw = CFBundleGetMainBundle();
        if raw.is_null() {
            return None;
        }
        // CFBundleGetMainBundle returns a borrowed reference; wrap it
        // without retaining and read the bundle identifier. `bundle`
        // intentionally doesn't drop the underlying CF object.
        let bundle = CFBundle::wrap_under_get_rule(raw);
        let id_ref = core_foundation::bundle::CFBundleGetIdentifier(bundle.as_concrete_TypeRef());
        if id_ref.is_null() {
            return None;
        }
        let id = CFString::wrap_under_get_rule(id_ref);
        let s = id.to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

/// `sw_vers -productVersion` — e.g. "14.5". Used in the
/// `platform_supported` message; failure falls back to "unknown".
fn sw_vers_product_version() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// `arm64` / `x86_64`. Matches the usual architecture labels.
fn arch_label() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use cu_core::health_report::{CheckStatus, HealthReportTool};
    use cu_core::tool::Tool;
    use std::sync::Arc;

    #[test]
    fn binary_version_always_passes() {
        let entry = check_binary_version();
        assert_eq!(entry.status, CheckStatus::Pass);
        assert!(
            entry.message.contains("brigadier-cu "),
            "message must include the binary version"
        );
    }

    #[test]
    fn platform_supported_carries_os_and_arch_data() {
        let entry = check_platform_supported();
        assert_eq!(entry.status, CheckStatus::Pass);
        let data = entry.data.expect("data block expected");
        assert!(data.os_version.is_some(), "os_version must be set");
        assert!(data.architecture.is_some(), "architecture must be set");
    }

    #[test]
    fn session_active_passes() {
        let entry = check_session_active();
        assert_eq!(entry.status, CheckStatus::Pass);
    }

    #[test]
    fn bundle_identity_in_test_host_fails_with_full_shape() {
        // The Rust test binary runs outside Brigadier Computer Use.app, so its
        // bundle id is either absent or not ai.brigadier.computer-use. Either
        // way the documented fail-mode shape applies: message + hint
        // + data + (when bid is present) bundle_identifier surfaced.
        //
        // This is the same fixture pattern the Swift PR used: the
        // test environment naturally reproduces the canonical
        // attribution-drift fail mode without any mocking.
        let entry = check_bundle_identity();
        assert_eq!(
            entry.status,
            CheckStatus::Fail,
            "expected bundle_identity to fail in the test host environment"
        );
        assert!(!entry.message.is_empty());
        assert!(
            entry.hint.is_some(),
            "fail entries must carry a remediation hint"
        );
        let data = entry.data.expect("fail entries must carry diagnostic data");
        // executable_path is always available when current_exe()
        // works; surface it so consumers can locate the wrong binary.
        assert!(
            data.executable_path.is_some(),
            "executable_path must be set so consumers can identify the wrong binary"
        );
    }

    #[test]
    fn external_brigadier_helper_bundle_identity_passes_without_standalone_hint() {
        let entry = check_bundle_identity_for_context(
            Some("ai.brigadier.dev.computer-use".to_owned()),
            "/Users/me/Library/Application Support/ai.brigadier.app/computer/Brigadier Computer Use.app/Contents/MacOS/brigadier-cu".to_owned(),
            true,
        );

        assert_eq!(entry.status, CheckStatus::Pass);
        assert!(
            entry
                .hint
                .as_deref()
                .is_none_or(|hint| !hint.contains("Brigadier Computer Use")),
            "a healthy host-owned helper must not recommend Brigadier Computer Use.app",
        );
    }

    // End-to-end through the dispatcher — checks every macOS canonical
    // name appears in the response and the response shape matches the
    // documented contract.
    #[tokio::test]
    async fn invoke_full_run_produces_macos_check_set() {
        let provider = Arc::new(MacosHealthProvider);
        let tool = HealthReportTool::new(provider);
        let result = tool.invoke(serde_json::json!({})).await;
        assert!(
            result.is_error.is_none(),
            "health_report must never set isError"
        );
        let structured = result.structured_content.expect("structured payload");
        assert_eq!(structured["platform"], "darwin");
        assert_eq!(structured["schema_version"], "1");
        let names: Vec<&str> = structured["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        // Every documented macOS check appears, in declared order.
        let expected: Vec<&str> = MACOS_CHECK_NAMES.to_vec();
        assert_eq!(names, expected);
    }
}
