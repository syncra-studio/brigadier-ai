//! Deadline tests. Every test drives time with `ManualClock`; none sleeps or
//! depends on wall time. A "provider" is the capture closure: it either
//! returns at once, blocks on a channel the test controls, or never returns
//! until the test releases it.

use super::*;
use std::sync::mpsc;
use std::sync::Weak;

/// Tests share the process-wide abandoned-worker counter, so they run one at
/// a time.
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Manual clock: time moves only on `advance`.
pub(crate) struct ManualClock {
    base: Instant,
    state: Mutex<ManualState>,
    parked_changed: Condvar,
}

#[derive(Default)]
struct ManualState {
    offset: Duration,
    park_calls: usize,
    signals: Vec<Weak<CaptureSignal>>,
}

impl ManualClock {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            base: Instant::now(),
            state: Mutex::new(ManualState::default()),
            parked_changed: Condvar::new(),
        })
    }

    pub(crate) fn advance(&self, by: Duration) {
        let signals = {
            let mut state = self.state.lock().unwrap();
            state.offset += by;
            state
                .signals
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>()
        };
        for signal in signals {
            signal.notify();
        }
    }

    /// Block until the waiter has entered `park` at least `count` times.
    pub(crate) fn wait_for_park_calls(&self, count: usize) {
        let state = self.state.lock().unwrap();
        let _state = self
            .parked_changed
            .wait_while(state, |state| state.park_calls < count)
            .unwrap();
    }
}

impl CaptureClock for ManualClock {
    fn now(&self) -> Instant {
        self.base + self.state.lock().unwrap().offset
    }

    fn park(&self, signal: &Arc<CaptureSignal>, seen: u64, deadline: Instant) {
        {
            let mut state = self.state.lock().unwrap();
            state.signals.push(Arc::downgrade(signal));
            state.park_calls += 1;
            self.parked_changed.notify_all();
            if self.base + state.offset >= deadline {
                return;
            }
        }
        signal.wait_changed(seen, None);
    }
}

/// Provider that blocks until the test sends a value, or forever.
fn gated_provider<T: Send + 'static>() -> (mpsc::Sender<T>, impl FnOnce() -> T + Send + 'static) {
    let (release, gate) = mpsc::channel::<T>();
    (release, move || {
        gate.recv().expect("test dropped the provider gate")
    })
}

const BUDGET: Duration = Duration::from_secs(10);

#[test]
fn never_completing_provider_returns_capture_timeout_at_the_injected_budget() {
    let _serial = serial();
    let baseline = abandoned_workers();
    let clock = ManualClock::new();
    let (release, provider) = gated_provider::<u32>();
    let (late_tx, late_rx) = mpsc::channel();

    let waiter_clock = Arc::clone(&clock);
    let waiter = std::thread::spawn(move || {
        bounded(
            CaptureOperation::ShareableContent,
            BUDGET,
            waiter_clock.as_ref(),
            provider,
            move |value| late_tx.send(value).unwrap(),
        )
    });

    clock.wait_for_park_calls(1);
    clock.advance(BUDGET - Duration::from_millis(1));
    // The waiter re-checks, sees time left, and parks again: no timeout yet.
    clock.wait_for_park_calls(2);
    assert!(!waiter.is_finished(), "must not time out before the budget");

    clock.advance(Duration::from_millis(1));
    let error = waiter
        .join()
        .unwrap()
        .expect_err("provider never completed");
    let timeout = CaptureTimeout::find(&error).expect("typed capture_timeout");
    assert_eq!(timeout.operation, CaptureOperation::ShareableContent);
    assert_eq!(timeout.elapsed, BUDGET);
    assert_eq!(timeout.budget, BUDGET);
    assert_eq!(timeout.reason, CaptureTimeoutReason::BudgetElapsed);
    assert_eq!(abandoned_workers(), baseline + 1);

    // Unblock the abandoned worker so it does not outlive the test.
    release.send(7).unwrap();
    assert_eq!(late_rx.recv().unwrap(), 7);
    assert_eq!(abandoned_workers(), baseline);
}

#[test]
fn provider_that_completes_in_time_returns_the_normal_result() {
    let _serial = serial();
    let clock = ManualClock::new();

    // Completes at once.
    let value = bounded(
        CaptureOperation::ScreenshotCapture,
        BUDGET,
        clock.as_ref(),
        || 42u32,
        |_| panic!("an in-time value must not reach on_late"),
    )
    .unwrap();
    assert_eq!(value, 42);

    // Completes while the waiter is parked, before the budget.
    let (release, provider) = gated_provider::<&'static str>();
    let waiter_clock = Arc::clone(&clock);
    let waiter = std::thread::spawn(move || {
        bounded(
            CaptureOperation::StreamStart,
            BUDGET,
            waiter_clock.as_ref(),
            provider,
            |_| panic!("an in-time value must not reach on_late"),
        )
    });
    clock.wait_for_park_calls(1);
    clock.advance(BUDGET - Duration::from_millis(1));
    release.send("frame").unwrap();
    assert_eq!(waiter.join().unwrap().unwrap(), "frame");
}

#[test]
fn late_completion_after_timeout_is_dropped_once_and_never_answers_twice() {
    let _serial = serial();
    let baseline = abandoned_workers();
    let clock = ManualClock::new();

    struct Probe(Arc<()>);
    let token = Arc::new(());
    let (release, provider) = gated_provider::<Probe>();
    let (late_tx, late_rx) = mpsc::channel::<usize>();

    let waiter_clock = Arc::clone(&clock);
    let waiter = std::thread::spawn(move || {
        bounded(
            CaptureOperation::StreamStart,
            BUDGET,
            waiter_clock.as_ref(),
            provider,
            move |late: Probe| {
                // Stand-in for "stop the stream that started too late".
                let holders = Arc::strong_count(&late.0);
                drop(late);
                late_tx.send(holders).unwrap();
            },
        )
    });
    clock.wait_for_park_calls(1);
    clock.advance(BUDGET);
    let error = waiter.join().unwrap().err().expect("timed out");
    assert!(CaptureTimeout::find(&error).is_some());

    release.send(Probe(Arc::clone(&token))).unwrap();
    // on_late ran exactly once with the late value...
    assert_eq!(late_rx.recv().unwrap(), 2);
    // ...and nothing else will: the handler and its sender are gone.
    assert!(
        late_rx.recv().is_err(),
        "late value delivered more than once"
    );
    // The late value was released, not leaked, and the counter recovered.
    assert_eq!(Arc::strong_count(&token), 1);
    assert_eq!(abandoned_workers(), baseline);
}

#[test]
fn stuck_workers_past_the_cap_fail_fast_without_calling_the_provider() {
    let _serial = serial();
    let baseline = abandoned_workers();
    let mut releases = Vec::new();
    let (late_tx, late_rx) = mpsc::channel::<()>();

    for _ in 0..(MAX_ABANDONED_WORKERS - baseline) {
        let clock = ManualClock::new();
        let (release, provider) = gated_provider::<()>();
        releases.push(release);
        let waiter_clock = Arc::clone(&clock);
        let late_tx = late_tx.clone();
        let waiter = std::thread::spawn(move || {
            bounded(
                CaptureOperation::DisplayImage,
                BUDGET,
                waiter_clock.as_ref(),
                provider,
                move |()| late_tx.send(()).unwrap(),
            )
        });
        clock.wait_for_park_calls(1);
        clock.advance(BUDGET);
        assert!(waiter.join().unwrap().is_err());
    }
    assert_eq!(abandoned_workers(), MAX_ABANDONED_WORKERS);
    let clock = ManualClock::new();

    let error = bounded(
        CaptureOperation::DisplayImage,
        BUDGET,
        clock.as_ref(),
        || panic!("provider must not run past the cap"),
        |()| {},
    )
    .err()
    .expect("cap reached");
    let timeout = CaptureTimeout::find(&error).unwrap();
    assert_eq!(
        timeout.reason,
        CaptureTimeoutReason::PreviousCapturesPending
    );
    assert_eq!(timeout.elapsed, Duration::ZERO);

    for release in releases {
        release.send(()).unwrap();
        late_rx.recv().unwrap();
    }
    assert_eq!(abandoned_workers(), baseline);
}

#[test]
fn provider_panic_is_reported_as_an_error_not_a_hang() {
    let _serial = serial();
    let clock = ManualClock::new();
    let error = bounded(
        CaptureOperation::WindowImage,
        BUDGET,
        clock.as_ref(),
        || -> u32 { panic!("sck exploded") },
        |_| {},
    )
    .err()
    .expect("panic surfaces as error");
    let panicked = error.downcast_ref::<CaptureWorkerPanicked>().unwrap();
    assert_eq!(panicked.operation, CaptureOperation::WindowImage);
    assert!(panicked.message.contains("sck exploded"));
}

#[test]
fn capture_timeout_survives_context_and_serializes_the_documented_shape() {
    let timeout = CaptureTimeout {
        operation: CaptureOperation::DisplayImage,
        elapsed: Duration::from_millis(10_000),
        budget: SCREENSHOT_BUDGET,
        reason: CaptureTimeoutReason::BudgetElapsed,
    };
    let error = anyhow::Error::from(timeout.clone()).context("Desktop screenshot failed");
    assert_eq!(CaptureTimeout::find(&error), Some(&timeout));
    assert!(format!("{error:#}").contains("capture_timeout: display_image"));

    let json = timeout.to_json();
    assert_eq!(json["code"], "capture_timeout");
    assert_eq!(json["operation"], "display_image");
    assert_eq!(json["elapsed_ms"], 10_000);
    assert_eq!(json["budget_ms"], 10_000);
    assert_eq!(json["reason"], "budget_elapsed");
    assert!(json["hint"].as_str().unwrap().contains("dialog"));
}
