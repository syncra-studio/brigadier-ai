//! Layer-3 focus-steal preventer — Rust port of Swift's
//! `SystemFocusStealPreventer.swift` plus the PR #1521 4-layer hardening
//! (closure scope, RAII lease, 5s monotonic deadline, 1s janitor).
//!
//! ## What this protects against
//!
//! `NSWorkspace.OpenConfiguration.activates = false` tells LaunchServices
//! "don't activate the target on launch". LaunchServices honors that.
//! What it does NOT do is stop the launched app from calling
//! `NSApp.activate(ignoringOtherApps:)` in its own
//! `applicationDidFinishLaunching`. Chrome, Electron, Safari, Calculator
//! all do exactly that — so a "background" launch flashes the target on
//! top of the user's work for a few frames.
//!
//! The preventer subscribes to
//! `NSWorkspace.didActivateApplicationNotification` and, when an activation
//! matches a registered suppression entry, immediately re-activates the
//! prior frontmost app on a background thread. AppKit's
//! `-[NSRunningApplication activateWithOptions:]` is documented thread-safe
//! — no main-thread hop required.
//!
//! ## Layered design (matches PR #1521)
//!
//! 1. **Closure API** — `with_suppression(target, restore_to, origin, f)`
//!    begins an entry, awaits `f`, ends the entry. Use this when the
//!    suppression scope is a single async block.
//! 2. **RAII API** — `begin_suppression(target, restore_to, origin)`
//!    returns a `SuppressionLease`. `Drop` ends the entry synchronously
//!    (no awaiting). Use this when the caller needs to hold the lease
//!    across multiple branches or when async cancellation may interrupt
//!    the closure path.
//! 3. **5s monotonic deadline** — every entry stamps an
//!    `Instant::now() + 5s`. The observer prunes expired entries before
//!    matching, so a leaked lease can't cause a stale entry to keep
//!    re-activating the prior frontmost app forever.
//! 4. **1s janitor** — a tokio interval task wakes up every second
//!    while the dispatcher is non-empty, prunes expired entries, and
//!    stops when the map drains. Re-starts when the next entry is
//!    added. Coordinated via `tokio::sync::watch`.
//!
//! ## Singleton
//!
//! `FocusStealPreventer::shared()` returns a process-wide
//! `Arc<FocusStealPreventer>`. The observer registration happens inside
//! `OnceLock::get_or_init`, so it's safe to call from any thread without
//! racing on observer install.
//!
//! ## Why a fresh background `NSOperationQueue` (not `mainQueue`)
//!
//! NSWorkspace's block-based observer fires on the queue you give it. If
//! the queue is `nil` (Swift default) or `mainQueue`, the block runs on
//! the main thread — which means it requires a live main run loop.
//! `brigadier-cu call` (one-shot subcommand) and `--no-overlay` mode don't
//! have one, so the activation observer would never fire. A fresh
//! background `NSOperationQueue` sidesteps that — the block runs on the
//! queue's own thread regardless of run-loop state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use objc2_app_kit::{
    NSApplicationActivationOptions, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification,
};
use objc2_foundation::NSOperationQueue;
use uuid::Uuid;

/// Per-entry deadline. After this much wall-clock time the dispatcher's
/// observer (and the janitor) treats the entry as leaked and prunes it
/// without firing. Mirrors Swift PR #1521.
const ENTRY_DEADLINE: Duration = Duration::from_secs(5);

/// Janitor tick interval. The task wakes up this often while the
/// dispatcher is non-empty and prunes expired entries.
const JANITOR_TICK: Duration = Duration::from_secs(1);

/// Identifier for a suppression. `with_suppression` and `begin_suppression`
/// hand one of these back; `end_suppression` consumes it.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct SuppressionHandle(Uuid);

/// Dispatcher-internal entry shape.
#[derive(Debug)]
struct Entry {
    /// `Some(pid)` matches only that pid's activations. `None` matches
    /// only an app launched after the entry began (one the action itself
    /// launched, whose pid isn't known yet), never an app that was already
    /// running: switching to one of those is the user's doing.
    target_pid: Option<i32>,
    /// Wall-clock start, compared with an activated app's launch date.
    created: SystemTime,
    /// Pid to restore focus to when an activation matches this entry.
    restore_to: i32,
    /// Monotonic deadline. After this, the entry is pruned without
    /// firing.
    deadline: Instant,
    /// Provenance for tracing — e.g. `"LaunchAppTool.pre"`.
    #[allow(dead_code)]
    origin: &'static str,
}

/// Singleton focus-steal preventer.
///
/// Constructed lazily on first `shared()` call. Owns the dispatcher state
/// (Sync via the inner Mutex); the NSWorkspace observer + queue are
/// intentionally retained-and-forgotten on install so their lifetime is
/// the whole process and we don't need to thread `!Send` Cocoa handles
/// through this struct.
pub struct FocusStealPreventer {
    dispatcher: Arc<Dispatcher>,
}

impl FocusStealPreventer {
    /// Return (or initialize on first call) the process-wide singleton.
    pub fn shared() -> Arc<Self> {
        static SINGLETON: OnceLock<Arc<FocusStealPreventer>> = OnceLock::new();
        SINGLETON
            .get_or_init(|| {
                let dispatcher = Arc::new(Dispatcher::new());
                install_observer(&dispatcher);
                Arc::new(FocusStealPreventer { dispatcher })
            })
            .clone()
    }

    /// Begin suppressing focus-steals targeting `target_pid` (or any pid
    /// when `None`, the wildcard). Returns a `SuppressionLease` whose
    /// `Drop` ends the entry synchronously.
    ///
    /// `restore_to` is the pid the preventer re-activates if it matches
    /// a notification. `origin` is a static label for tracing.
    pub fn begin_suppression(
        target_pid: Option<i32>,
        restore_to: i32,
        origin: &'static str,
    ) -> SuppressionLease {
        Self::begin_suppression_for(target_pid, restore_to, origin, ENTRY_DEADLINE, None)
    }

    /// [`Self::begin_suppression`] for a longer watch than the default
    /// deadline, e.g. a launch the app may answer only seconds later.
    /// `action_started`, when the action itself launched the target, is
    /// when the action began: the app's activations are then always sent
    /// back, also right after the user's own input, since they can't be the
    /// user's own switch.
    pub fn begin_suppression_for(
        target_pid: Option<i32>,
        restore_to: i32,
        origin: &'static str,
        lasting: Duration,
        action_started: Option<SystemTime>,
    ) -> SuppressionLease {
        let shared = Self::shared();
        let handle = shared.dispatcher.add_since(
            target_pid,
            restore_to,
            origin,
            lasting,
            action_started.unwrap_or_else(SystemTime::now),
        );
        SuppressionLease {
            handle,
            dispatcher: Arc::clone(&shared.dispatcher),
            released: false,
        }
    }

    /// Run `f` with a suppression entry active. Equivalent to
    /// `begin_suppression(...)` + run `f` + drop the lease — but expressed
    /// as a single async call site.
    pub async fn with_suppression<R, F, Fut>(
        target_pid: Option<i32>,
        restore_to: i32,
        origin: &'static str,
        f: F,
    ) -> R
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = R>,
    {
        let _lease = Self::begin_suppression(target_pid, restore_to, origin);
        f().await
    }
}

/// Begin-suppression convenience that bounces through the singleton.
pub fn begin_suppression(
    target_pid: Option<i32>,
    restore_to: i32,
    origin: &'static str,
) -> SuppressionLease {
    FocusStealPreventer::begin_suppression(target_pid, restore_to, origin)
}

/// RAII lease. `Drop` ends the entry synchronously, so the entry is
/// removed even if a future is cancelled mid-await.
pub struct SuppressionLease {
    handle: SuppressionHandle,
    dispatcher: Arc<Dispatcher>,
    released: bool,
}

impl SuppressionLease {
    /// Explicit release. Useful if the caller wants to drop the lease
    /// before its scope ends without taking the `Drop` path.
    pub fn release(mut self) {
        self.dispatcher.remove(self.handle);
        self.released = true;
    }
}

impl Drop for SuppressionLease {
    fn drop(&mut self) {
        if !self.released {
            self.dispatcher.remove(self.handle);
        }
    }
}

// ── Dispatcher ──────────────────────────────────────────────────────────────

/// Holds the suppression entries plus the janitor lifecycle.
///
/// `entries` is a `HashMap<Uuid, Entry>` so add/remove are O(1) by handle.
/// Lookups by `(target_pid, restore_to)` during a notification are O(N) —
/// N is at most a handful of in-flight launches at a time so a linear
/// scan is fine.
pub(crate) struct Dispatcher {
    entries: Mutex<HashMap<Uuid, Entry>>,
    /// `true` while the janitor task should keep running. The janitor
    /// loop watches for transitions to detect when to start/stop.
    janitor_active: tokio::sync::watch::Sender<bool>,
    janitor_started: Mutex<bool>,
}

impl Dispatcher {
    fn new() -> Self {
        let (tx, _rx) = tokio::sync::watch::channel(false);
        Self {
            entries: Mutex::new(HashMap::new()),
            janitor_active: tx,
            janitor_started: Mutex::new(false),
        }
    }

    /// Add an entry, return its handle. Always attempts to start the
    /// janitor task — `kick_janitor()` is idempotent and is the only
    /// reliable path to recover if the very first add happened before
    /// a tokio runtime was ready. Gating the kick on "map was empty"
    /// (as we used to) lost the janitor permanently in that case:
    /// subsequent adds would skip the kick and the janitor never
    /// started, leaving deadline-reaping entirely up to the
    /// `snapshot_matches` reap fallback (only fires on an activation).
    #[cfg(test)]
    fn add(
        self: &Arc<Self>,
        target_pid: Option<i32>,
        restore_to: i32,
        origin: &'static str,
        lasting: Duration,
    ) -> SuppressionHandle {
        self.add_since(target_pid, restore_to, origin, lasting, SystemTime::now())
    }

    /// [`Self::add`] for an action that began at `created`: an app launched
    /// since then counts as launched by the action.
    fn add_since(
        self: &Arc<Self>,
        target_pid: Option<i32>,
        restore_to: i32,
        origin: &'static str,
        lasting: Duration,
        created: SystemTime,
    ) -> SuppressionHandle {
        let id = Uuid::new_v4();
        let entry = Entry {
            target_pid,
            restore_to,
            created,
            deadline: Instant::now() + lasting,
            origin,
        };
        {
            let mut guard = self.entries.lock().unwrap();
            guard.insert(id, entry);
        }
        // Always kick — idempotent if the task is already running.
        self.kick_janitor();
        // Signal the janitor that there's work to do (it will start a
        // fresh tokio interval on the next tick).
        let _ = self.janitor_active.send(true);
        SuppressionHandle(id)
    }

    /// Remove an entry. When the map drains to empty, signals the janitor
    /// to stop until the next add.
    fn remove(&self, handle: SuppressionHandle) {
        let now_empty = {
            let mut guard = self.entries.lock().unwrap();
            guard.remove(&handle.0);
            guard.is_empty()
        };
        if now_empty {
            let _ = self.janitor_active.send(false);
        }
    }

    /// Snapshot the entries (cloned to a small Vec) — used by tests
    /// and the activation handler to evaluate matches without holding
    /// the lock across the restore call.
    /// The restore_to pids of the entries an activation matches, each with
    /// whether the activated app was launched after that entry began.
    fn snapshot_matches(
        &self,
        activated_pid: i32,
        launched_at: Option<SystemTime>,
    ) -> Vec<(i32, bool)> {
        let mut guard = self.entries.lock().unwrap();
        // Reap expired entries first — keeps the dispatcher honest even
        // if the janitor hasn't ticked yet.
        let now = Instant::now();
        guard.retain(|_, e| e.deadline > now);
        guard
            .values()
            .filter(|e| {
                match e.target_pid {
                    Some(p) => p == activated_pid,
                    // Wildcard: only an app the action launched (never the
                    // restore_to pid, so we don't fight ourselves).
                    None => {
                        activated_pid != e.restore_to
                            && launched_at.is_some_and(|at| at >= e.created)
                    }
                }
            })
            .map(|e| (e.restore_to, launched_at.is_some_and(|at| at >= e.created)))
            .collect()
    }

    /// Number of entries (for tests).
    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Reap entries whose deadline is past. Returns the number reaped.
    fn reap_expired(&self) -> usize {
        let mut guard = self.entries.lock().unwrap();
        let now = Instant::now();
        let before = guard.len();
        guard.retain(|_, e| e.deadline > now);
        let after = guard.len();
        let reaped = before - after;
        if after == 0 && reaped > 0 {
            let _ = self.janitor_active.send(false);
        }
        reaped
    }

    /// Start the janitor task on the current tokio runtime (idempotent).
    ///
    /// Safe to call from `add()` on every entry — returns immediately
    /// when the task is already up. If the spawn cannot proceed (no
    /// tokio runtime available — e.g. the first `add()` raced the
    /// binary's runtime init), the `started` flag is intentionally left
    /// `false` so the next add from a tokio-aware caller retries.
    /// Without that retry path the janitor could go permanently
    /// un-spawned and deadline-reaping would degrade to the
    /// `snapshot_matches` reap fallback (only runs on an activation).
    fn kick_janitor(self: &Arc<Self>) {
        let mut started = self.janitor_started.lock().unwrap();
        if *started {
            return;
        }
        // If there's no tokio runtime available (e.g. the binary is in
        // the middle of an init path that runs before Tokio is up), skip
        // — the next add from a tokio-aware caller will retry. We do NOT
        // set `*started = true` in this branch so the retry actually
        // takes the spawn path.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        // Mark started ONLY after a successful `tokio::spawn`. The
        // spawn itself is infallible under the current API but we still
        // sequence the flag update after the spawn so any future
        // panic-from-spawn path would leave `started = false` and the
        // next add would retry.
        let weak = Arc::downgrade(self);
        let mut rx = self.janitor_active.subscribe();
        tokio::spawn(async move {
            loop {
                // Block until the dispatcher is non-empty.
                if !*rx.borrow_and_update() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                    continue;
                }
                let mut tick = tokio::time::interval(JANITOR_TICK);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                tick.tick().await; // immediate first tick
                loop {
                    tokio::select! {
                        _ = tick.tick() => {
                            let Some(d) = weak.upgrade() else { return };
                            let _ = d.reap_expired();
                            if d.len() == 0 {
                                // Map drained — break to outer select, wait
                                // for next add.
                                break;
                            }
                        }
                        ch = rx.changed() => {
                            if ch.is_err() { return; }
                            // Active flag may have flipped; loop top will
                            // re-check via `borrow_and_update`.
                            break;
                        }
                    }
                }
            }
        });
        *started = true;
    }
}

// ── Observer registration ────────────────────────────────────────────────────

/// Register the NSWorkspace.didActivateApplicationNotification observer.
///
/// The returned token and the queue are intentionally `mem::forget`-leaked
/// — the singleton is process-lifetime, so we never tear the observer
/// down. Forgetting avoids having to thread `!Send` `Retained<...>`
/// handles through `FocusStealPreventer` (which lives in `Arc<...>` /
/// `OnceLock<...>` and therefore needs to be `Send + Sync`).
fn install_observer(dispatcher: &Arc<Dispatcher>) {
    use block2::RcBlock;
    use objc2_foundation::NSNotification;
    use std::ptr::NonNull;

    let ws = unsafe { NSWorkspace::sharedWorkspace() };
    let center = unsafe { ws.notificationCenter() };

    // Fresh background NSOperationQueue. Critical: with `nil` queue,
    // AppKit delivers synchronously on the posting thread (typically main);
    // with `mainQueue`, the block requires a running main run loop. A
    // fresh queue runs the block on a private background thread no matter
    // what run loop the binary has up.
    let queue = unsafe { NSOperationQueue::new() };
    // setMaxConcurrentOperationCount: 1 means activations are processed
    // serially — they're cheap so contention isn't a worry, but serial
    // processing keeps the restore order deterministic if two come in
    // back to back.
    unsafe { queue.setMaxConcurrentOperationCount(1) };

    let dispatcher_clone = Arc::clone(dispatcher);
    let block = RcBlock::new(move |note_ptr: NonNull<NSNotification>| {
        // SAFETY: AppKit gives us a borrowed NSNotification for the
        // duration of the block. We don't escape the reference.
        let note = unsafe { note_ptr.as_ref() };
        handle_activation(&dispatcher_clone, note);
    });

    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidActivateApplicationNotification),
            None,
            Some(&queue),
            &block,
        )
    };

    // Intentionally leak both — the observer needs to outlive any
    // particular `Arc<FocusStealPreventer>` and the singleton has
    // process lifetime.
    std::mem::forget(token);
    std::mem::forget(queue);
}

/// Match a single activation notification against the dispatcher and,
/// for each matching entry, re-activate the entry's `restore_to` pid.
///
/// Runs on the observer queue's background thread — safe to call
/// blocking system APIs.
fn handle_activation(dispatcher: &Arc<Dispatcher>, note: &objc2_foundation::NSNotification) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let activated_pid: i32 = unsafe {
        let info = match note.userInfo() {
            Some(i) => i,
            None => return,
        };
        // userInfo[NSWorkspaceApplicationKey] -> NSRunningApplication*.
        // We go through a raw msg_send to avoid Retained generic
        // bookkeeping for the cross-cast.
        let app_ptr: *mut AnyObject = msg_send![&*info, objectForKey: NSWorkspaceApplicationKey];
        if app_ptr.is_null() {
            return;
        }
        let pid: libc::pid_t = msg_send![app_ptr, processIdentifier];
        pid as i32
    };

    let app =
        unsafe { NSRunningApplication::runningApplicationWithProcessIdentifier(activated_pid) };
    let launched_at = app
        .as_ref()
        .and_then(|app| unsafe { app.launchDate() })
        .map(|date| {
            SystemTime::UNIX_EPOCH
                + Duration::from_secs_f64(unsafe { date.timeIntervalSince1970() }.max(0.0))
        });
    let matches = dispatcher.snapshot_matches(activated_pid, launched_at);
    if matches.is_empty() {
        return;
    }
    // An app the action itself launched can't be the user's own switch.
    let launched_by_action = matches.iter().all(|(_, fresh)| *fresh);
    let bundle = app
        .as_ref()
        .and_then(|app| unsafe { app.bundleIdentifier() })
        .map(|id| id.to_string());
    let regular = app.as_ref().is_some_and(|app| unsafe {
        app.activationPolicy() == objc2_app_kit::NSApplicationActivationPolicy::Regular
    });
    let idle = if launched_by_action {
        f64::INFINITY
    } else {
        seconds_since_user_input()
    };
    if let Some(reason) = leave_in_front(bundle.as_deref(), regular, idle) {
        tracing::info!(activated_pid, ?bundle, reason, "focus-steal: left in front");
        return;
    }
    for (pid, _) in matches {
        restore_focus(pid);
    }
}

/// System prompts and panels that must never be sent back: a password or
/// Touch ID prompt, a permission or notification alert.
const SYSTEM_UI: &[&str] = &[
    "com.apple.LocalAuthentication.UIAgent",
    "com.apple.SecurityAgent",
    "com.apple.UserNotificationCenter",
    "com.apple.notificationcenterui",
    "com.apple.coreservices.uiagent",
    "com.apple.CoreLocationAgent",
    "com.apple.accessibility.universalAccessAuthWarn",
    "com.apple.loginwindow",
    "com.apple.dock",
    "com.apple.controlcenter",
    "com.apple.Spotlight",
    "com.apple.systemuiserver",
];

/// Seconds a real person needs to have been idle before an activation can
/// be taken for an app stealing the front rather than for their own switch.
const USER_INPUT_GRACE_SECS: f64 = 1.0;

/// Why an activation that matched a suppression is left alone, if it is:
/// it followed the user's own keyboard or pointer input, or it is a system
/// prompt or panel rather than a regular app.
fn leave_in_front(
    bundle: Option<&str>,
    regular_app: bool,
    secs_since_user_input: f64,
) -> Option<&'static str> {
    if bundle.is_some_and(|id| SYSTEM_UI.contains(&id)) {
        return Some("system prompt");
    }
    if !regular_app {
        return Some("not a regular app");
    }
    if secs_since_user_input < USER_INPUT_GRACE_SECS {
        return Some("user input");
    }
    None
}

/// Seconds since the last keyboard, pointer or trackpad event from the
/// hardware (events the helper posts to an app don't count).
fn seconds_since_user_input() -> f64 {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
    }
    const HID_SYSTEM_STATE: i32 = 1;
    const ANY_INPUT_EVENT: u32 = u32::MAX;
    unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) }
}

/// Put `pid` back in front if it's still running: through the WindowServer
/// call the foreground assist restores with (back within ~50 ms when
/// measured, where an app sent back by an AppKit activation stayed in front
/// about a second), else by asking AppKit, which is documented thread-safe.
fn restore_focus(pid: i32) {
    if crate::input::skylight::set_front_pid(pid) {
        return;
    }
    unsafe {
        if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
            let _ = app.activateWithOptions(NSApplicationActivationOptions(0));
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// These tests exercise the pure-Rust dispatcher half — no Cocoa
// observers, no real notifications. They run under `cargo test
// -p platform-macos focus_steal::`.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Dispatcher::add returns a handle, the entry is reachable by
    /// match, and remove() drops it.
    #[test]
    fn dispatcher_add_match_remove() {
        let d = Arc::new(Dispatcher::new());
        let h = d.add(Some(42), 7, "test.add", ENTRY_DEADLINE);
        assert_eq!(d.len(), 1);
        let matches = d.snapshot_matches(42, None);
        assert_eq!(matches, vec![(7, false)]);
        // Non-matching pid: no restore candidates.
        assert!(d.snapshot_matches(99, None).is_empty());
        d.remove(h);
        assert_eq!(d.len(), 0);
    }

    /// Wildcard entries (`target_pid = None`) match only an app launched
    /// after the entry began, never one that was already running (the
    /// user switching to it), and never the entry's own restore_to pid.
    #[test]
    fn wildcard_matches_only_an_app_launched_during_the_action() {
        let d = Arc::new(Dispatcher::new());
        let before = SystemTime::now() - Duration::from_secs(60);
        let _h = d.add(None, 7, "test.wild", ENTRY_DEADLINE);
        let after = SystemTime::now() + Duration::from_millis(1);
        assert_eq!(d.snapshot_matches(99, Some(after)), vec![(7, true)]);
        assert!(d.snapshot_matches(99, Some(before)).is_empty());
        assert!(d.snapshot_matches(99, None).is_empty());
        assert!(d.snapshot_matches(7, Some(after)).is_empty());
    }

    /// A targeted watch dated from before the launch counts the app as
    /// launched by the action; one begun after it does not.
    #[test]
    fn a_watch_dated_from_the_action_marks_its_launch_as_the_actions() {
        let d = Arc::new(Dispatcher::new());
        let action_started = SystemTime::now() - Duration::from_secs(2);
        let launched = SystemTime::now() - Duration::from_secs(1);
        let h = d.add_since(Some(5), 1, "test.since", ENTRY_DEADLINE, action_started);
        assert_eq!(d.snapshot_matches(5, Some(launched)), vec![(1, true)]);
        d.remove(h);
        let _h = d.add(Some(5), 1, "test.now", ENTRY_DEADLINE);
        assert_eq!(d.snapshot_matches(5, Some(launched)), vec![(1, false)]);
    }

    /// A longer watch outlives the default deadline; the default expires.
    #[test]
    fn a_longer_watch_outlives_the_default_deadline() {
        let d = Arc::new(Dispatcher::new());
        let _short = d.add(Some(5), 1, "test.short", Duration::from_millis(0));
        let _long = d.add(Some(5), 2, "test.long", Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(d.snapshot_matches(5, None), vec![(2, false)]);
    }

    /// The user's own input, system prompts and non-regular apps are
    /// never sent back.
    #[test]
    fn user_switches_and_system_prompts_are_left_in_front() {
        assert_eq!(
            leave_in_front(Some("com.apple.LocalAuthentication.UIAgent"), true, 60.0),
            Some("system prompt")
        );
        assert_eq!(
            leave_in_front(Some("com.apple.UserNotificationCenter"), true, 60.0),
            Some("system prompt")
        );
        assert_eq!(
            leave_in_front(Some("x.agent"), false, 60.0),
            Some("not a regular app")
        );
        assert_eq!(
            leave_in_front(Some("company.browser"), true, 0.3),
            Some("user input")
        );
        assert_eq!(leave_in_front(Some("com.apple.TextEdit"), true, 5.0), None);
        assert_eq!(
            leave_in_front(Some("com.apple.TextEdit"), true, f64::INFINITY),
            None
        );
    }

    /// Lease Drop is the standard remove path.
    #[test]
    fn lease_drop_removes_entry() {
        // Use a private dispatcher to avoid singleton coupling.
        let d = Arc::new(Dispatcher::new());
        let h = d.add(Some(1), 2, "test.lease", ENTRY_DEADLINE);
        let lease = SuppressionLease {
            handle: h,
            dispatcher: Arc::clone(&d),
            released: false,
        };
        assert_eq!(d.len(), 1);
        drop(lease);
        assert_eq!(d.len(), 0);
    }

    /// Explicit release() short-circuits the Drop path.
    #[test]
    fn lease_release_removes_entry() {
        let d = Arc::new(Dispatcher::new());
        let h = d.add(Some(1), 2, "test.lease", ENTRY_DEADLINE);
        let lease = SuppressionLease {
            handle: h,
            dispatcher: Arc::clone(&d),
            released: false,
        };
        lease.release();
        assert_eq!(d.len(), 0);
    }

    /// Force a leaked entry whose deadline is already past, then call
    /// reap_expired and snapshot_matches — both must purge it.
    #[test]
    fn deadline_reaps_leaked_entry() {
        let d = Arc::new(Dispatcher::new());
        // Insert a handle manually with a past deadline.
        let id = Uuid::new_v4();
        {
            let mut guard = d.entries.lock().unwrap();
            guard.insert(
                id,
                Entry {
                    target_pid: Some(42),
                    restore_to: 7,
                    created: SystemTime::now(),
                    deadline: Instant::now() - Duration::from_secs(1),
                    origin: "test.leak",
                },
            );
        }
        assert_eq!(d.len(), 1);
        // snapshot_matches reaps expired entries before matching.
        let matches = d.snapshot_matches(42, None);
        assert!(matches.is_empty(), "expired entry should not fire");
        assert_eq!(d.len(), 0, "snapshot_matches should purge expired");
    }

    /// Janitor lifecycle: starts on first add, stops when empty,
    /// restarts on next add. Spin up a tokio runtime to host the task.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn janitor_starts_stops_restarts() {
        let d = Arc::new(Dispatcher::new());
        // First add → janitor starts.
        let h1 = d.add(Some(1), 2, "test.j1", ENTRY_DEADLINE);
        d.kick_janitor();
        // Give the janitor task time to spin up.
        tokio::time::sleep(Duration::from_millis(50)).await;
        // The dispatcher should still hold the entry.
        assert_eq!(d.len(), 1);
        // Now remove → janitor goes idle.
        d.remove(h1);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(d.len(), 0);
        // Add again — same kick, same lifecycle. (start_janitor is
        // idempotent — already-started task picks up new adds via watch.)
        let _h2 = d.add(Some(3), 4, "test.j2", ENTRY_DEADLINE);
        d.kick_janitor();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(d.len(), 1);
    }

    /// Verifies the CodeRabbit #2 fix: `add()` always calls
    /// `kick_janitor()`, regardless of whether the map was empty.
    ///
    /// Scenario: first `add()` happens outside a tokio runtime —
    /// `kick_janitor()` short-circuits via `Handle::try_current()` and
    /// leaves `started = false`. A second `add()` from a tokio-aware
    /// caller (the more common case in practice) must retry the spawn.
    /// The old code skipped the kick because the map was non-empty,
    /// stranding the janitor forever.
    #[test]
    fn add_always_kicks_janitor_after_initial_runtime_miss() {
        let d = Arc::new(Dispatcher::new());
        // Outside any tokio runtime — kick_janitor's `try_current` guard
        // returns Err, the function returns without setting started.
        let h1 = d.add(Some(1), 2, "test.no_runtime", ENTRY_DEADLINE);
        assert_eq!(d.len(), 1);
        assert!(
            !*d.janitor_started.lock().unwrap(),
            "kick without a runtime must leave started=false so the next \
             add retries"
        );

        // Now spin up a tokio runtime and add a second entry. The fix
        // is that this *second* add still calls kick_janitor (the old
        // code skipped because the map was already non-empty). Verify
        // by asserting started flips to true.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build current-thread runtime");
        let d_in = Arc::clone(&d);
        rt.block_on(async move {
            let _h2 = d_in.add(Some(3), 4, "test.with_runtime", ENTRY_DEADLINE);
            assert_eq!(d_in.len(), 2);
            assert!(
                *d_in.janitor_started.lock().unwrap(),
                "second add from inside the runtime must retry the spawn \
                 (regression guard for CodeRabbit #2)"
            );
        });

        // Clean up so the dispatcher doesn't outlive the runtime with
        // a still-armed entry — not strictly needed (Dispatcher is
        // `Send + Sync` and the spawned task holds only a Weak ref),
        // but keeps the test self-contained.
        d.remove(h1);
    }

    /// Snapshot ordering doesn't matter, but the restore pid set
    /// must contain every match. Multiple concurrent suppressions
    /// targeting the same pid should both fire.
    #[test]
    fn multiple_entries_match_independently() {
        let d = Arc::new(Dispatcher::new());
        let _a = d.add(Some(42), 1, "test.m1", ENTRY_DEADLINE);
        let _b = d.add(Some(42), 2, "test.m2", ENTRY_DEADLINE);
        let matches = d.snapshot_matches(42, None);
        assert_eq!(matches.len(), 2);
        // Set equality — order is HashMap-dependent.
        assert!(matches.contains(&(1, false)));
        assert!(matches.contains(&(2, false)));
    }
}
