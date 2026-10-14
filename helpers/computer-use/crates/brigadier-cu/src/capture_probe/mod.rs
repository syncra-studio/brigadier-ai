//! The live screen-capture readiness probe behind the host's
//! `verify_screen_capture` request, and the bounded waits it runs under
//! ([`deadline`]).

pub mod deadline;

use deadline::CaptureTimeout;

/// Perform the prompt-capable live ScreenCaptureKit readiness check for an
/// embedding host's explicit onboarding flow.
///
/// This is deliberately not registered as an MCP tool. Only the daemon's
/// host-authenticated control request can reach it, so agents cannot raise
/// Tahoe's private-window-picker consent outside the host UI.
///
/// `Err` means macOS did not answer the probe in time (typically a pending
/// system dialog); that is unknown readiness, not a missing grant.
pub fn verify_screen_capture_ready() -> Result<bool, CaptureTimeout> {
    screen_recording_capturable()
}

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
type ScreenCaptureProbe = Result<bool, CaptureTimeout>;

fn screen_recording_capturable() -> ScreenCaptureProbe {
    use screencapturekit::{
        prelude::{SCContentFilter, SCShareableContent, SCStreamConfiguration},
        screenshot_manager::SCScreenshotManager,
    };

    use deadline::CaptureOperation;

    if crate::permissions::tcc_off_for_tests() {
        return Ok(false);
    }
    verify_screen_capture_with(
        || {
            probe_step(
                deadline::run(
                    CaptureOperation::ShareableContent,
                    deadline::SHAREABLE_CONTENT_BUDGET,
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
                deadline::run(
                    CaptureOperation::ScreenshotCapture,
                    deadline::SCREENSHOT_BUDGET,
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

#[cfg(test)]
mod tests {
    use super::*;
    use deadline::CaptureOperation;

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
            reason: deadline::CaptureTimeoutReason::BudgetElapsed,
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
}
