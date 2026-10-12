use async_trait::async_trait;
use cu_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

use crate::capture_deadline::CaptureTimeout;

use crate::permissions::status::{
    accessibility_granted, request_accessibility, request_screen_recording,
    screen_recording_granted,
};

pub struct CheckPermissionsTool;

/// (A) Real ScreenCaptureKit capability probe — what THIS process can
/// actually capture right now, independent of the CGPreflight cache.
///
/// `CGPreflightScreenCaptureAccess()` (used by `screen_recording_granted`)
/// answers from a per-process cache that goes stale after `tccutil reset`
/// and is unreliable for CLI / child processes — the same finding Peekaboo
/// documents. Display enumeration alone is not sufficient on macOS Tahoe:
/// `SCShareableContent::get()` can return displays while the separate
/// direct-capture consent alert is still waiting for a decision. Readiness
/// therefore requires one real frame from `SCScreenshotManager`.
/// Result of the live probe: `Ok(capturable)` when macOS answered, or the
/// typed timeout when it did not. A timeout is not evidence that the grant is
/// missing (a pending system dialog blocks capture too), so it is reported as
/// `capture_timeout`, never as `capturable: false`.
pub(super) type ScreenCaptureProbe = Result<bool, CaptureTimeout>;

pub(super) fn screen_recording_capturable() -> ScreenCaptureProbe {
    use screencapturekit::{
        prelude::{SCContentFilter, SCShareableContent, SCStreamConfiguration},
        screenshot_manager::SCScreenshotManager,
    };

    use crate::capture_deadline::{self, CaptureOperation};

    verify_screen_capture_with(
        || {
            probe_step(
                capture_deadline::run(
                    CaptureOperation::ShareableContent,
                    capture_deadline::SHAREABLE_CONTENT_BUDGET,
                    || {
                        Ok(SCShareableContent::get()
                            .ok()
                            .and_then(|content| content.displays().into_iter().next()))
                    },
                ),
                None,
                "display enumeration",
            )
        },
        |display| {
            let display = display.clone();
            probe_step(
                capture_deadline::run(
                    CaptureOperation::ScreenshotCapture,
                    capture_deadline::SCREENSHOT_BUDGET,
                    move || {
                        let filter = SCContentFilter::create()
                            .with_display(&display)
                            .with_excluding_windows(&[])
                            .build();
                        let configuration =
                            SCStreamConfiguration::new().with_width(1).with_height(1);
                        Ok(SCScreenshotManager::capture_image(&filter, &configuration)
                            .is_ok_and(|image| image.width() > 0 && image.height() > 0))
                    },
                ),
                false,
                "frame capture",
            )
        },
    )
}

/// Keep a capture timeout typed; any other bounded-call failure (worker
/// panic, thread spawn failure) reads as the step's `fallback`.
fn probe_step<T>(result: anyhow::Result<T>, fallback: T, step: &str) -> Result<T, CaptureTimeout> {
    result.or_else(|error| match CaptureTimeout::find(&error) {
        Some(timeout) => Err(timeout.clone()),
        None => {
            tracing::warn!(%error, "screen capture probe: {step} failed");
            Ok(fallback)
        }
    })
}

fn verify_screen_capture_with<Display>(
    load_display: impl FnOnce() -> Result<Option<Display>, CaptureTimeout>,
    capture_frame: impl FnOnce(&Display) -> Result<bool, CaptureTimeout>,
) -> ScreenCaptureProbe {
    match load_display()? {
        Some(display) => capture_frame(&display),
        None => Ok(false),
    }
}

/// Run the ScreenCaptureKit probe only on an explicitly prompt-capable path.
/// `SCShareableContent::get()` can itself register/raise TCC, so `None` is the
/// only truthful answer when a silent or host-owned flow skips that probe.
fn maybe_screen_recording_capture_probe(
    should_probe: bool,
    probe: impl FnOnce() -> ScreenCaptureProbe,
) -> Option<ScreenCaptureProbe> {
    should_probe.then(probe)
}

/// (B) Which TCC identity the booleans in this response reflect.
///
/// macOS attributes Accessibility / Screen-Recording to the *responsible
/// process* (the LaunchServices launching app), not the executable path.
/// So `check_permissions` answered in-process reflects:
///   - the **BrigadierCu daemon** (`ai.brigadier.computer-use`) when this process is
///     its own responsible process — the real driver status.
///   - the **calling app** otherwise — e.g. the terminal/IDE that spawned
///     `brigadier-cu call …`. That grant is NOT the driver's, which is why a
///     standalone check can read `true` while `tccutil … ai.brigadier.computer-use`
///     reports no record.
fn permission_source() -> serde_json::Value {
    let pid = unsafe { libc::getpid() };
    let ppid = unsafe { libc::getppid() };
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok())
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_default();
    let disclaimed = std::env::var_os(cu_core::RESPONSIBILITY_DISCLAIMED_ENV).is_some();
    let embedded = cu_core::embedded_mode();
    let host_bundle_id = std::env::var(cu_core::HOST_BUNDLE_ID_ENV).unwrap_or_default();
    let bundle_identifier = super::health_report::current_bundle_identifier();
    permission_source_for_context(
        pid,
        ppid,
        &exe,
        disclaimed,
        embedded,
        &host_bundle_id,
        bundle_identifier.as_deref(),
    )
}

fn permission_source_for_context(
    pid: libc::pid_t,
    ppid: libc::pid_t,
    exe: &str,
    disclaimed: bool,
    embedded: bool,
    host_bundle_id: &str,
    bundle_identifier: Option<&str>,
) -> serde_json::Value {
    // Embedded mode: the driver is a child in a host app's responsibility
    // chain, so the probes already answer for the host's TCC identity.
    // This branch only ever downgrades attribution (host, never
    // driver-daemon), so the caller-controlled env var can't spoof an
    // elevated identity. `host_bundle_id` is advisory, not a trust signal.
    if embedded {
        return serde_json::json!({
            "attribution": "host",
            "host_bundle_id": host_bundle_id,
            "embedded": true,
            "pid": pid,
            "responsible_ppid": ppid,
            "executable": exe,
            "disclaim_env": disclaimed,
            "note": "Embedded mode: these booleans reflect the HOST app's TCC \
                     grant (the driver is a child in the host's responsibility \
                     chain). No separate driver grant exists or is needed. If a \
                     permission is NOT granted, the host app must request it — \
                     the driver never raises its own prompt.",
        });
    }
    // The trustworthy, non-spoofable signal is the executable path: a caller
    // can't run from inside the code-signed `Brigadier Computer Use.app` bundle without
    // controlling that install. The disclaim env var is caller-controlled, so
    // it is treated only as a corroborating signal that explains why a
    // bundle-resident daemon has `ppid != 1` (it re-exec'd itself with
    // responsibility disclaim, so launchd is no longer its parent). On its own
    // — outside the bundle — the env var must NOT grant daemon attribution, or
    // a caller could pre-set it and spoof the TCC source. Fail closed to
    // "caller" whenever the bundle signal is absent.
    let inside_app_bundle = exe.contains(".app/Contents/MacOS/");
    let is_responsible_app = inside_app_bundle
        && bundle_identifier.is_some_and(|identifier| !identifier.is_empty())
        && (ppid == 1 || disclaimed);

    let (attribution, note) = if is_responsible_app
        && bundle_identifier == Some(super::health_report::CANONICAL_BUNDLE_ID)
    {
        (
            "driver-daemon",
            "These booleans reflect the BrigadierCu daemon's own TCC identity \
             (ai.brigadier.computer-use) because this process is its own responsible \
             process.",
        )
    } else if is_responsible_app {
        (
            "helper-daemon",
            "These booleans reflect this helper application's own TCC identity. Permission setup belongs to the embedding host; do not run the standalone brigadier-cu permission flow.",
        )
    } else {
        (
            "caller",
            "These booleans reflect the TCC identity of the app that launched \
             this process (e.g. your terminal/IDE), NOT the BrigadierCu daemon \
             (ai.brigadier.computer-use). A standalone check can read `true` here while \
             `tccutil … ai.brigadier.computer-use` reports no record. To grant for the \
             driver, run `brigadier-cu permissions grant`.",
        )
    };

    serde_json::json!({
        "attribution": attribution,
        "pid": pid,
        "responsible_ppid": ppid,
        "executable": exe,
        "bundle_identifier": bundle_identifier,
        "disclaim_env": disclaimed,
        "note": note,
    })
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        // Matches Swift `CheckPermissionsTool.swift` description verbatim.
        name: "check_permissions".into(),
        description: "Report TCC permission status for Accessibility and Screen Recording. \
            By default also raises the system permission dialogs for any missing grants — \
            Apple's request APIs are no-ops when the grant is already active, so this is \
            safe to call repeatedly. Pass {\"prompt\": false} for a purely read-only \
            status check.\n\n\
            Returns: `accessibility` + `screen_recording` (booleans from the TCC \
            preflight APIs), `screen_recording_capturable` (true/false only when a \
            live ScreenCaptureKit probe ran; null for prompt:false, embedded, or \
            external permission flows or when the probe timed out), \
            `screen_recording_probe_performed`, `screen_recording_capture_error` \
            (a `capture_timeout` object when macOS did not answer the probe, usually \
            because a system dialog is waiting; the grants are not the cause), and \
            `source` (which TCC identity the \
            booleans reflect: the responsible daemon app vs the launching terminal/IDE). \
            macOS attributes grants to the responsible process, so a standalone call \
            from a terminal reports the terminal's grants, not the driver's.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "boolean",
                    "description": "Raise the system permission prompts for missing grants. Default true.",
                }
            },
            "additionalProperties": false,
        }),
        // Not read_only because the default path may raise a modal dialog
        // (mirrors Swift annotation `readOnlyHint: false`).
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for CheckPermissionsTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cu_core::tool_args::ArgsExt;
        // Default to prompting — same default + rationale as Swift.
        // Embedded mode hard-disables prompting regardless of the arg (the
        // host owns the grant flow). This and the startup gate are the only
        // `request_*` call sites, so both being gated makes prompts
        // unreachable when embedded.
        let should_prompt = args.bool_or("prompt", true)
            && !cu_core::embedded_mode()
            && !super::health_report::external_permission_flow_enabled();
        if should_prompt {
            let _ = request_accessibility();
            let _ = request_screen_recording();
        }
        let accessibility = accessibility_granted();
        let screen_recording = screen_recording_granted();
        // (A) Authoritative live probe — see `screen_recording_capturable`.
        //
        // CRITICAL: `SCShareableContent::get()` REGISTERS this process with TCC
        // and RAISES the Screen Recording system prompt when the grant is
        // missing. That is a real side effect, so it must only happen when the
        // caller opted into prompting (`prompt:true`). A read-only status query
        // (`prompt:false`) — e.g. a host app refreshing the helper's status when
        // an agent session merely starts — MUST stay silent; running the live
        // probe there pops a permission dialog before the user has done anything
        // with computer use. When we're not allowed to prompt, report capture
        // status as unknown instead of mislabelling the preflight boolean as a
        // live result.
        let screen_recording_capturable =
            maybe_screen_recording_capture_probe(should_prompt, screen_recording_capturable);
        // (B) Which identity the booleans above belong to.
        let source = permission_source();

        permission_result(
            accessibility,
            screen_recording,
            screen_recording_capturable,
            source,
        )
    }
}

fn permission_result(
    accessibility: bool,
    screen_recording: bool,
    probe: Option<ScreenCaptureProbe>,
    source: serde_json::Value,
) -> ToolResult {
    let screen_recording_capturable = probe
        .as_ref()
        .and_then(|probe| probe.as_ref().ok().copied());
    let capture_timeout = probe.as_ref().and_then(|probe| probe.as_ref().err());
    let is_caller = source.get("attribution").and_then(|v| v.as_str()) == Some("caller");
    // Text format mirrors Swift 1:1:
    //   "✅ Accessibility: granted.\n✅ Screen Recording: granted."
    let ax_prefix = if accessibility { "✅" } else { "❌" };
    let sr_prefix = if screen_recording { "✅" } else { "❌" };
    let ax_state = if accessibility {
        "granted"
    } else {
        "NOT granted"
    };
    let sr_state = if screen_recording {
        "granted"
    } else {
        "NOT granted"
    };
    let mut summary = format!(
        "{ax_prefix} Accessibility: {ax_state}.\n{sr_prefix} Screen Recording: {sr_state}."
    );
    // Flag a preflight/probe disagreement (the false-positive tell).
    if screen_recording && screen_recording_capturable == Some(false) {
        summary.push_str(
            "\n⚠️  Screen Recording reads granted but a live capture probe failed — \
             the grant likely belongs to a different process, not this one.",
        );
    }
    if let Some(timeout) = capture_timeout {
        summary.push_str(&format!(
            "\n⏳ Live capture probe: {timeout} The permission state above is \
             unchanged; do not re-grant it."
        ));
    }
    // Make the attribution explicit when answering for a host or caller
    // (not the daemon).
    if source.get("attribution").and_then(|v| v.as_str()) == Some("host") {
        summary.push_str(
            "\nℹ️  Embedded mode: status reflects the HOST app's TCC grant. \
             If a permission is missing, the host must request it — the \
             driver will not prompt.",
        );
    }
    if is_caller {
        summary.push_str(
            "\nℹ️  Status reflects the launching app's TCC identity, not the BrigadierCu \
             daemon (ai.brigadier.computer-use). See `source` for details.",
        );
    }

    ToolResult::text(summary).with_structured(serde_json::json!({
        "accessibility":               accessibility,
        "screen_recording":            screen_recording,
        "screen_recording_capturable": screen_recording_capturable,
        "screen_recording_probe_performed": probe.is_some(),
        "screen_recording_capture_error": capture_timeout.map(CaptureTimeout::to_json),
        "source":                      source,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_deadline::{CaptureOperation, CaptureTimeout};

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::permissions::test_env_lock()
    }

    /// Set/remove `var`, returning the original for restore. Callers must
    /// hold `env_lock()`.
    fn swap_env(var: &str, value: Option<&str>) -> Option<std::ffi::OsString> {
        let original = std::env::var_os(var);
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
        original
    }

    fn restore_env(var: &str, original: Option<std::ffi::OsString>) {
        match original {
            Some(value) => std::env::set_var(var, value),
            None => std::env::remove_var(var),
        }
    }

    #[test]
    fn disclaim_env_var_alone_does_not_grant_daemon_attribution() {
        // The disclaim env var is caller-controlled, so on its own it must not
        // make `check_permissions` claim the booleans reflect the daemon's TCC
        // identity. Daemon attribution additionally requires the binary to live
        // inside the code-signed `Brigadier Computer Use.app` bundle — the test runner does
        // not, so even with the env var present we must fail closed to "caller".
        let _guard = env_lock();
        let name = cu_core::RESPONSIBILITY_DISCLAIMED_ENV;
        let original = swap_env(name, Some("1"));
        let embedded = swap_env(cu_core::EMBEDDED_ENV, None);

        let source = permission_source();
        assert_eq!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("caller"),
            "env-var presence alone must not yield daemon attribution"
        );

        restore_env(cu_core::EMBEDDED_ENV, embedded);
        restore_env(name, original);
    }

    #[test]
    fn embedded_mode_reports_host_attribution() {
        let _guard = env_lock();
        let embedded = swap_env(cu_core::EMBEDDED_ENV, Some("1"));
        let host = swap_env(cu_core::HOST_BUNDLE_ID_ENV, Some("com.example.host"));

        let source = permission_source();
        assert_eq!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("host"),
        );
        assert_eq!(
            source.get("host_bundle_id").and_then(|v| v.as_str()),
            Some("com.example.host"),
        );
        assert_eq!(source.get("embedded").and_then(|v| v.as_bool()), Some(true));

        restore_env(cu_core::HOST_BUNDLE_ID_ENV, host);
        restore_env(cu_core::EMBEDDED_ENV, embedded);
    }

    #[test]
    fn embedded_plus_disclaim_env_never_yields_daemon_attribution() {
        // Both caller-controlled env vars together must still not produce
        // "driver-daemon" — embedded mode may only DOWNGRADE attribution.
        let _guard = env_lock();
        let embedded = swap_env(cu_core::EMBEDDED_ENV, Some("1"));
        let disclaim = swap_env(cu_core::RESPONSIBILITY_DISCLAIMED_ENV, Some("1"));

        let source = permission_source();
        assert_eq!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("host"),
        );

        restore_env(cu_core::RESPONSIBILITY_DISCLAIMED_ENV, disclaim);
        restore_env(cu_core::EMBEDDED_ENV, embedded);
    }

    #[test]
    fn embedded_env_requires_exact_value_one() {
        let _guard = env_lock();
        let embedded = swap_env(cu_core::EMBEDDED_ENV, Some("true"));
        let source = permission_source();
        assert_ne!(
            source.get("attribution").and_then(|v| v.as_str()),
            Some("host"),
            "only BRIGADIER_CU_EMBEDDED=1 may enable embedded mode"
        );
        restore_env(cu_core::EMBEDDED_ENV, embedded);
    }

    #[test]
    fn brigadier_helper_reports_its_own_daemon_identity() {
        let source = permission_source_for_context(
            42,
            1,
            "/Users/me/Library/Application Support/ai.brigadier.app/computer/Brigadier Computer Use.app/Contents/MacOS/brigadier-cu",
            true,
            false,
            "",
            Some("ai.brigadier.dev.computer-use"),
        );

        assert_eq!(
            source.get("attribution").and_then(|value| value.as_str()),
            Some("helper-daemon"),
        );
        assert_eq!(
            source
                .get("bundle_identifier")
                .and_then(|value| value.as_str()),
            Some("ai.brigadier.dev.computer-use"),
        );
        assert!(
            !source["note"]
                .as_str()
                .unwrap_or_default()
                .contains("permissions grant"),
            "a host-owned helper must never recommend the standalone permission flow",
        );
    }

    #[test]
    fn silent_check_reports_capture_unknown_without_running_probe() {
        let probe_called = std::cell::Cell::new(false);
        let capturable = maybe_screen_recording_capture_probe(false, || {
            probe_called.set(true);
            Ok(true)
        });
        assert_eq!(capturable, None);
        assert!(
            !probe_called.get(),
            "silent status must not call SCShareableContent"
        );

        let result = permission_result(
            true,
            true,
            capturable,
            serde_json::json!({ "attribution": "helper-daemon" }),
        );
        let structured = result
            .structured_content
            .as_ref()
            .expect("structured status");
        assert!(structured["screen_recording_capturable"].is_null());
        assert_eq!(structured["screen_recording_probe_performed"], false);
        let text = result
            .content
            .iter()
            .find_map(|content| match content {
                cu_core::protocol::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(!text.contains("live capture probe failed"));
    }

    #[test]
    fn capture_readiness_requires_a_real_frame_after_display_enumeration() {
        let captures = std::cell::Cell::new(0);
        assert_eq!(
            verify_screen_capture_with(
                || Ok(None::<u8>),
                |_| {
                    captures.set(captures.get() + 1);
                    Ok(true)
                }
            ),
            Ok(false)
        );
        assert_eq!(captures.get(), 0);

        assert_eq!(
            verify_screen_capture_with(
                || Ok(Some(7_u8)),
                |_| {
                    captures.set(captures.get() + 1);
                    Ok(false)
                }
            ),
            Ok(false)
        );
        assert_eq!(captures.get(), 1);

        assert_eq!(
            verify_screen_capture_with(
                || Ok(Some(7_u8)),
                |_| {
                    captures.set(captures.get() + 1);
                    Ok(true)
                }
            ),
            Ok(true)
        );
        assert_eq!(captures.get(), 2);
    }

    fn timeout(operation: CaptureOperation) -> CaptureTimeout {
        CaptureTimeout {
            operation,
            elapsed: std::time::Duration::from_secs(15),
            budget: std::time::Duration::from_secs(15),
            reason: crate::capture_deadline::CaptureTimeoutReason::BudgetElapsed,
        }
    }

    #[test]
    fn probe_timeout_propagates_from_enumeration_and_from_capture() {
        let captured = std::cell::Cell::new(false);
        let enumeration = timeout(CaptureOperation::ShareableContent);
        assert_eq!(
            verify_screen_capture_with(
                || Err::<Option<u8>, _>(enumeration.clone()),
                |_| {
                    captured.set(true);
                    Ok(true)
                }
            ),
            Err(enumeration)
        );
        assert!(!captured.get(), "no capture after an enumeration timeout");

        let frame = timeout(CaptureOperation::ScreenshotCapture);
        assert_eq!(
            verify_screen_capture_with(|| Ok(Some(7_u8)), |_| Err(frame.clone())),
            Err(frame)
        );
    }

    #[test]
    fn bounded_probe_errors_keep_timeouts_and_map_other_failures_to_the_fallback() {
        let hung = anyhow::Error::from(timeout(CaptureOperation::ShareableContent));
        assert_eq!(
            probe_step(Err::<Option<u8>, _>(hung), None, "enumeration"),
            Err(timeout(CaptureOperation::ShareableContent))
        );
        assert_eq!(
            probe_step(
                Err::<bool, _>(anyhow::anyhow!("worker died")),
                false,
                "frame"
            ),
            Ok(false)
        );
        assert_eq!(probe_step(Ok(true), false, "frame"), Ok(true));
    }

    #[test]
    fn probe_timeout_is_reported_as_capture_timeout_not_as_not_capturable() {
        let result = permission_result(
            true,
            true,
            Some(Err(timeout(CaptureOperation::ScreenshotCapture))),
            serde_json::json!({ "attribution": "helper-daemon" }),
        );
        let structured = result
            .structured_content
            .as_ref()
            .expect("structured status");
        // Unknown, never `false`: a false value tells users to re-grant a
        // permission that is already granted.
        assert!(structured["screen_recording_capturable"].is_null());
        assert_eq!(structured["screen_recording_probe_performed"], true);
        let error = &structured["screen_recording_capture_error"];
        assert_eq!(error["code"], "capture_timeout");
        assert_eq!(error["operation"], "screenshot_capture");
        assert_eq!(error["elapsed_ms"], 15_000);
        assert!(error["hint"].as_str().unwrap().contains("dialog"));

        let text = result
            .content
            .iter()
            .find_map(|content| match content {
                cu_core::protocol::Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(text.contains("capture_timeout"), "{text}");
        assert!(!text.contains("live capture probe failed"), "{text}");
    }

    #[test]
    fn completed_probe_has_no_capture_error() {
        let result = permission_result(
            true,
            true,
            Some(Ok(true)),
            serde_json::json!({ "attribution": "helper-daemon" }),
        );
        let structured = result.structured_content.as_ref().unwrap();
        assert!(structured["screen_recording_capture_error"].is_null());
    }

    #[test]
    fn live_check_reports_probe_true_or_false_and_warns_only_on_false() {
        for (live, should_warn) in [(true, false), (false, true)] {
            let capturable = maybe_screen_recording_capture_probe(true, || Ok(live));
            assert_eq!(capturable, Some(Ok(live)));
            let result = permission_result(
                true,
                true,
                capturable,
                serde_json::json!({ "attribution": "helper-daemon" }),
            );
            let structured = result
                .structured_content
                .as_ref()
                .expect("structured status");
            assert_eq!(structured["screen_recording_capturable"], live);
            assert_eq!(structured["screen_recording_probe_performed"], true);
            let text = result
                .content
                .iter()
                .find_map(|content| match content {
                    cu_core::protocol::Content::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .unwrap_or_default();
            assert_eq!(text.contains("live capture probe failed"), should_warn);
        }
    }
}
