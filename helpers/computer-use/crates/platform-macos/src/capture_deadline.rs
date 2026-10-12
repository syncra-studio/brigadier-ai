//! Bounded waits for ScreenCaptureKit and other screen-capture calls.
//!
//! ScreenCaptureKit (and the legacy CoreGraphics / `screencapture` capture
//! paths, which macOS routes through the same capture service) can block
//! forever: a system consent alert, a "bypass the private window picker"
//! prompt, or a wedged `replayd` leaves the completion handler pending with
//! no error. A tool request that awaits such a call must not hang with it.
//!
//! [`bounded`] runs one capture operation on a dedicated worker thread and
//! waits at most `budget` on an injectable [`CaptureClock`]. When the budget
//! elapses, the caller gets a typed [`CaptureTimeout`] and the worker is
//! abandoned: when the system call eventually returns, its value goes to the
//! `on_late` handler (for example to stop a stream that started too late) or
//! is dropped. The caller is answered exactly once, and a late completion
//! never touches the caller's state.
//!
//! A blocked system call cannot be cancelled, so an abandoned worker thread
//! stays parked until macOS answers. [`MAX_ABANDONED_WORKERS`] caps how many
//! such threads can exist; past the cap a new capture fails immediately with
//! the same typed error instead of spawning another stuck thread.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Budget for `SCShareableContent` enumeration. A healthy call answers in
/// well under a second; the first call after login or after `replayd`
/// restarts can take a few seconds. 15 s leaves wide margin for that cold
/// start and still answers well inside a typical 60 s client timeout.
pub const SHAREABLE_CONTENT_BUDGET: Duration = Duration::from_secs(15);
/// Budget for a single still capture (`SCScreenshotManager`, CoreGraphics
/// window/display image, `screencapture`). A full-display native-resolution
/// frame takes tens to hundreds of milliseconds; 10 s only trips when the
/// capture service is blocked.
pub const SCREENSHOT_BUDGET: Duration = Duration::from_secs(10);
/// Budget for `SCStream::start_capture`. Starting a stream negotiates with
/// `replayd` and can raise the same consent alerts as enumeration.
pub const STREAM_START_BUDGET: Duration = Duration::from_secs(15);
/// Budget for `SCStream::stop_capture`. Stopping a recording finalises the
/// mp4 (moov atom) before returning, which grows with recording length.
pub const STREAM_STOP_BUDGET: Duration = Duration::from_secs(20);

/// Machine-readable error code carried by every [`CaptureTimeout`].
pub const CAPTURE_TIMEOUT_CODE: &str = "capture_timeout";
/// Cap on worker threads parked inside capture calls that already timed out.
pub const MAX_ABANDONED_WORKERS: usize = 4;

const HINT: &str = "macOS did not answer the screen-capture request. A system dialog or a \
    Screen Recording permission prompt may be waiting for a click on this Mac; answer it, \
    then retry.";

/// The capture call that did not answer in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOperation {
    ShareableContent,
    ScreenshotCapture,
    WindowImage,
    DisplayImage,
    StreamStart,
    StreamStop,
}

impl CaptureOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ShareableContent => "shareable_content",
            Self::ScreenshotCapture => "screenshot_capture",
            Self::WindowImage => "window_image",
            Self::DisplayImage => "display_image",
            Self::StreamStart => "stream_start",
            Self::StreamStop => "stream_stop",
        }
    }
}

/// Why the capture did not produce a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTimeoutReason {
    /// The call ran for the full budget without an answer.
    BudgetElapsed,
    /// Too many earlier calls are still blocked; this one was not started.
    PreviousCapturesPending,
}

/// Typed timeout error. Travels inside `anyhow::Error`; tool layers use
/// [`CaptureTimeout::find`] to turn it into a structured response.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "capture_timeout: {} did not complete within {} ms (waited {} ms). {}",
    operation.as_str(),
    budget.as_millis(),
    elapsed.as_millis(),
    HINT
)]
pub struct CaptureTimeout {
    pub operation: CaptureOperation,
    pub elapsed: Duration,
    pub budget: Duration,
    pub reason: CaptureTimeoutReason,
}

impl CaptureTimeout {
    pub fn code(&self) -> &'static str {
        CAPTURE_TIMEOUT_CODE
    }

    pub fn hint(&self) -> &'static str {
        HINT
    }

    /// Find a `CaptureTimeout` anywhere in an `anyhow` error chain.
    pub fn find(error: &anyhow::Error) -> Option<&CaptureTimeout> {
        error
            .chain()
            .find_map(|cause| cause.downcast_ref::<CaptureTimeout>())
    }

    /// Structured payload for tool and daemon responses.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "code": CAPTURE_TIMEOUT_CODE,
            "message": self.to_string(),
            "operation": self.operation.as_str(),
            "elapsed_ms": self.elapsed.as_millis() as u64,
            "budget_ms": self.budget.as_millis() as u64,
            "reason": match self.reason {
                CaptureTimeoutReason::BudgetElapsed => "budget_elapsed",
                CaptureTimeoutReason::PreviousCapturesPending => "previous_captures_pending",
            },
            "hint": HINT,
        })
    }
}

/// A capture call that failed without timing out.
#[derive(Debug, thiserror::Error)]
#[error("{operation} worker panicked: {message}", operation = operation.as_str())]
pub struct CaptureWorkerPanicked {
    pub operation: CaptureOperation,
    pub message: String,
}

/// Wake-up channel between a capture worker, the clock, and the waiter.
/// A generation counter prevents lost wake-ups between check and park.
#[derive(Default)]
pub struct CaptureSignal {
    generation: Mutex<u64>,
    changed: Condvar,
}

impl CaptureSignal {
    pub fn generation(&self) -> u64 {
        *self
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn notify(&self) {
        let mut generation = self
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *generation = generation.wrapping_add(1);
        self.changed.notify_all();
    }

    /// Park until the generation moves past `seen`, or `timeout` (real time)
    /// passes when one is given.
    pub fn wait_changed(&self, seen: u64, timeout: Option<Duration>) {
        let guard = self
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match timeout {
            Some(timeout) => {
                let (guard, _timed_out) = self
                    .changed
                    .wait_timeout_while(guard, timeout, |generation| *generation == seen)
                    .unwrap_or_else(PoisonError::into_inner);
                drop(guard);
            }
            None => {
                let guard = self
                    .changed
                    .wait_while(guard, |generation| *generation == seen)
                    .unwrap_or_else(PoisonError::into_inner);
                drop(guard);
            }
        }
    }
}

/// Time source for capture deadlines. Production uses [`SystemCaptureClock`];
/// tests inject a manual clock so no test depends on wall time.
pub trait CaptureClock: Send + Sync {
    fn now(&self) -> Instant;
    /// Park until `signal` moves past generation `seen` or until `deadline`
    /// on this clock. Spurious returns are allowed; the caller re-checks.
    fn park(&self, signal: &Arc<CaptureSignal>, seen: u64, deadline: Instant);
}

/// Monotonic wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemCaptureClock;

impl CaptureClock for SystemCaptureClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn park(&self, signal: &Arc<CaptureSignal>, seen: u64, deadline: Instant) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        signal.wait_changed(seen, Some(remaining));
    }
}

enum Slot<T> {
    Pending,
    Ready(T),
    Panicked(String),
    Abandoned,
}

struct Shared<T> {
    slot: Mutex<Slot<T>>,
    signal: Arc<CaptureSignal>,
}

static ABANDONED_WORKERS: AtomicUsize = AtomicUsize::new(0);

/// Number of worker threads still blocked in a capture call that timed out.
pub fn abandoned_workers() -> usize {
    ABANDONED_WORKERS.load(Ordering::Acquire)
}

/// Run `work` with the production clock; a late value is dropped.
pub fn run<T, F>(operation: CaptureOperation, budget: Duration, work: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    bounded(operation, budget, &SystemCaptureClock, work, drop)?
}

/// Run `work` on a worker thread and wait at most `budget` on `clock`.
///
/// Returns the work's value, or `Err(CaptureTimeout)` (as `anyhow::Error`)
/// when the budget elapses first. A value produced after the timeout is
/// passed to `on_late` on the worker thread and never reaches the caller.
pub fn bounded<T, F, L>(
    operation: CaptureOperation,
    budget: Duration,
    clock: &dyn CaptureClock,
    work: F,
    on_late: L,
) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
    L: FnOnce(T) + Send + 'static,
{
    let started = clock.now();
    if abandoned_workers() >= MAX_ABANDONED_WORKERS {
        return Err(CaptureTimeout {
            operation,
            elapsed: Duration::ZERO,
            budget,
            reason: CaptureTimeoutReason::PreviousCapturesPending,
        }
        .into());
    }

    let shared = Arc::new(Shared {
        slot: Mutex::new(Slot::Pending),
        signal: Arc::new(CaptureSignal::default()),
    });
    let worker_shared = Arc::clone(&shared);
    std::thread::Builder::new()
        .name(format!("brigadier-cu-capture-{}", operation.as_str()))
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(work));
            let mut slot = worker_shared
                .slot
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match (&*slot, outcome) {
                (Slot::Abandoned, outcome) => {
                    drop(slot);
                    ABANDONED_WORKERS.fetch_sub(1, Ordering::AcqRel);
                    tracing::warn!(
                        operation = operation.as_str(),
                        "capture completed after its timeout; result discarded"
                    );
                    if let Ok(value) = outcome {
                        on_late(value);
                    }
                }
                (_, Ok(value)) => {
                    *slot = Slot::Ready(value);
                    drop(slot);
                    worker_shared.signal.notify();
                }
                (_, Err(payload)) => {
                    *slot = Slot::Panicked(panic_message(payload.as_ref()));
                    drop(slot);
                    worker_shared.signal.notify();
                }
            }
        })
        .map_err(|error| anyhow::anyhow!("could not start capture worker: {error}"))?;

    let deadline = started + budget;
    loop {
        let seen = shared.signal.generation();
        {
            let mut slot = shared.slot.lock().unwrap_or_else(PoisonError::into_inner);
            match std::mem::replace(&mut *slot, Slot::Pending) {
                Slot::Ready(value) => return Ok(value),
                Slot::Panicked(message) => {
                    return Err(CaptureWorkerPanicked { operation, message }.into())
                }
                Slot::Pending | Slot::Abandoned => {}
            }
            let now = clock.now();
            if now >= deadline {
                *slot = Slot::Abandoned;
                ABANDONED_WORKERS.fetch_add(1, Ordering::AcqRel);
                drop(slot);
                tracing::warn!(
                    operation = operation.as_str(),
                    budget_ms = budget.as_millis() as u64,
                    "capture timed out"
                );
                return Err(CaptureTimeout {
                    operation,
                    elapsed: now.saturating_duration_since(started),
                    budget,
                    reason: CaptureTimeoutReason::BudgetElapsed,
                }
                .into());
            }
        }
        clock.park(&shared.signal, seen, deadline);
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_owned())
}

#[cfg(test)]
#[path = "capture_deadline_tests.rs"]
pub(crate) mod tests;
