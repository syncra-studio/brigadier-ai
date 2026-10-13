//! Window-change detector — Rust port of Swift's
//! `WindowChangeDetector` (`libs/brigadier-cu/Sources/BrigadierCuServer/Tools/WindowChangeDetector.swift`).
//!
//! ## What this does
//!
//! Action tools (click, type_text, hotkey, …) on a backgrounded app can
//! trigger window/foreground side-effects: a "Sign In" button opens a
//! modal sheet, a Safari link spawns a new tab, an autocomplete dropdown
//! pops a helper window. The Rust port mirrors Swift's
//! snapshot → action → detect cycle so tool results can:
//!
//! 1. Surface the side-effect to the agent (one-line suffix on the
//!    tool result, matching Swift verbatim).
//! 2. Arm a **wildcard** focus-steal suppression entry that covers the
//!    full snapshot→detect window. Wildcards (`target_pid = None`)
//!    catch any activation other than the prior frontmost — so even an
//!    app we didn't know about (Safari activating because a UTM Gallery
//!    link routed to it) is suppressed before the first compositor
//!    frame.
//!
//! ## Usage
//!
//! ```ignore
//! // Callers capture frontmost BEFORE the snapshot so the wildcard
//! // suppressor and the snapshot's recorded frontmost agree on the
//! // pid to restore to — avoids a race where another app activates
//! // between the caller's `frontmost_pid()` and the detector's own.
//! let prior_front = apps::frontmost_pid();
//! let snapshot = WindowChangeDetector::snapshot(prior_front);
//! // … perform action …
//! let changes = snapshot.detect();
//! // changes.result_suffix() — append to ToolResult text.
//! ```
//!
//! Dropping the `Snapshot` ends the suppression lease (RAII). `detect()`
//! also drops the lease before returning — the lease's `Drop` is
//! idempotent so explicit-detect + later-drop is safe.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::apps;
use crate::focus_steal::{self, SuppressionLease};
use crate::windows::{self, WindowInfo};

/// One window that appeared between `snapshot()` and `detect()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowEvent {
    pub window_id: u32,
    pub pid: i32,
    pub app_name: String,
    pub title: String,
}

/// Categorical diff entry. We mirror Swift which only emits
/// `WindowEvent` rows for *new* windows — closed/changed never appear
/// in the result suffix — but keep them as enum variants for future
/// extensibility and so unit tests can pin down the diff semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowChange {
    Opened(WindowEvent),
    Closed { window_id: u32 },
}

/// State captured immediately before the action fires.
///
/// Holds:
/// - `window_ids` — the set of visible layer-0 window IDs at snapshot
///   time. `detect()` diffs against this.
/// - `front_pid` — the OS frontmost pid at snapshot time. `detect()`
///   reports whether a *different* pid became frontmost. The wildcard
///   suppressor in `focus_steal` will normally restore the original
///   front before `detect()`'s poll loop observes the change, so this
///   field is best-effort.
/// - `_lease` — the wildcard suppression lease. Dropping the snapshot
///   ends suppression. Held inside `Option` so `detect()` can take it
///   and drop early.
pub struct Snapshot {
    window_ids: HashSet<u32>,
    front_pid: Option<i32>,
    _lease: Option<SuppressionLease>,
    taken_at: Instant,
    /// Agent session key (the agent cursor key); guards and late windows
    /// are kept per session.
    session: String,
}

/// Result of `detect()` — what changed during the action window.
#[derive(Debug, Clone)]
pub struct Changes {
    pub new_windows: Vec<WindowEvent>,
    pub foreground_changed: bool,
    /// Windows that an earlier action opened after its tool result was
    /// returned (seen when that action's guard window settled).
    pub late_new_windows: Vec<WindowEvent>,
}

impl Changes {
    pub fn no_change() -> Self {
        Self {
            new_windows: Vec::new(),
            foreground_changed: false,
            late_new_windows: Vec::new(),
        }
    }

    /// True when we found evidence that the action triggered a cross-app
    /// side-effect that required (or would have required) a foreground
    /// restore. Matches Swift's `Changes.needsRestore`.
    pub fn needs_restore(&self) -> bool {
        self.foreground_changed || !self.new_windows.is_empty()
    }

    /// One-liner summary to append to a tool result, or empty string
    /// when nothing interesting happened.
    ///
    /// Format mirrors Swift `WindowChangeDetector.Changes.resultSuffix`
    /// **verbatim** so MCP callers that key off the suffix wording
    /// don't need a per-binary special case.
    pub fn result_suffix(&self) -> String {
        let mut suffix = if !self.new_windows.is_empty() {
            format!(
                "\n\n🪟 Action opened new window(s): {}.",
                summarize_windows(&self.new_windows)
            )
        } else if self.foreground_changed {
            "\n\n🔀 Action caused a different app to become frontmost.".to_string()
        } else {
            String::new()
        };
        if !self.late_new_windows.is_empty() {
            suffix.push_str(&format!(
                "\n\n🪟 An earlier action opened new window(s) after it returned: {}.",
                summarize_windows(&self.late_new_windows)
            ));
        }
        suffix
    }
}

/// "App (\"Title\", ...); Other" — grouped by app name in stable order.
fn summarize_windows(windows: &[WindowEvent]) -> String {
    let mut by_app: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for w in windows {
        by_app.entry(&w.app_name).or_default().push(&w.title);
    }
    by_app
        .into_iter()
        .map(|(app, titles)| {
            let titles: Vec<String> = titles
                .into_iter()
                .filter(|t| !t.is_empty())
                .map(|t| format!("\"{t}\""))
                .collect();
            if titles.is_empty() {
                app.to_string()
            } else {
                format!("{app} ({})", titles.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Default poll deadline — new windows triggered by a click typically
/// appear within ~200ms on macOS; 1.0s gives the wildcard suppressor
/// time to fire and settle.
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Default inter-poll interval. Matches Swift's 50ms.
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Public API. Mirrors Swift `enum WindowChangeDetector` — no state of
/// its own; all state lives inside the returned `Snapshot`.
pub struct WindowChangeDetector;

impl WindowChangeDetector {
    /// Capture the current window set + frontmost pid and arm the
    /// wildcard focus-steal suppressor. Call immediately before
    /// dispatching the action.
    ///
    /// `prior_front` is the frontmost pid the **caller** already
    /// observed — typically captured one line earlier via
    /// `apps::frontmost_pid()` for the surrounding `focus_guard`
    /// lease. We use the caller's value (not a fresh re-read) so the
    /// wildcard suppressor's `restore_to` matches what the focus-guard
    /// lease saw; a race where another app became frontmost between
    /// the caller's read and this method would otherwise leave the
    /// two leases targeting different pids.
    ///
    /// Returns `Snapshot`. Drop ends suppression (via the held
    /// `SuppressionLease`); call `Snapshot::detect()` to consume the
    /// snapshot and get a `Changes` summary.
    ///
    /// Safe to call from any thread — `CGWindowListCopyWindowInfo` is
    /// documented as thread-safe.
    pub fn snapshot(prior_front: Option<i32>) -> Snapshot {
        Self::capture(prior_front, true, "")
    }

    /// [`Self::snapshot`] for an agent session (the agent cursor key), so the
    /// guard and late windows of this action stay with that session.
    pub fn snapshot_for_session(prior_front: Option<i32>, session: &str) -> Snapshot {
        Self::capture(prior_front, true, session)
    }

    /// Capture the same before-state without arming reactive focus suppression.
    /// Foreground delivery owns its temporary activation and restoration, so a
    /// wildcard lease would race the target while the action is settling.
    pub fn snapshot_without_suppression(prior_front: Option<i32>) -> Snapshot {
        Self::capture(prior_front, false, "")
    }

    /// [`Self::snapshot_without_suppression`] for an agent session.
    pub fn snapshot_without_suppression_for_session(
        prior_front: Option<i32>,
        session: &str,
    ) -> Snapshot {
        Self::capture(prior_front, false, session)
    }

    fn capture(prior_front: Option<i32>, suppress_focus: bool, session: &str) -> Snapshot {
        // This session's next action is about to fire: settle its previous
        // guard first so those late windows are attributed to that action.
        let previous = lock(&GUARDS).flush(session);
        if let Some(previous) = previous {
            settle(session, previous);
        }

        let window_ids: HashSet<u32> = windows::visible_windows()
            .into_iter()
            .filter(|w| w.layer == 0)
            .map(|w| w.window_id)
            .collect();

        // Arm wildcard suppression — covers snapshot → detect window.
        // restore_to = caller-captured frontmost; target = wildcard
        // (any other pid). If there's no frontmost (rare — screensaver,
        // login window), we skip the lease; foreground-change tracking
        // still runs.
        let lease = prior_front.filter(|_| suppress_focus).map(|restore_to| {
            focus_steal::begin_suppression(
                None, // wildcard
                restore_to,
                "WindowChangeDetector.snapshot",
            )
        });

        Snapshot {
            window_ids,
            front_pid: prior_front,
            _lease: lease,
            taken_at: Instant::now(),
            session: session.to_owned(),
        }
    }
}

impl Snapshot {
    /// Frontmost pid at snapshot time, if any.
    pub fn front_pid(&self) -> Option<i32> {
        self.front_pid
    }

    /// Poll for up to `DEFAULT_TIMEOUT` for new windows or a
    /// foreground-app change. Returns as soon as a change is detected
    /// or the timeout elapses.
    ///
    /// Consumes the snapshot — the wildcard suppression lease is
    /// dropped when this returns (covers the full action + detection
    /// window).
    pub fn detect(self) -> Changes {
        self.detect_with(DEFAULT_TIMEOUT, DEFAULT_POLL_INTERVAL)
    }

    /// Async wrapper around `detect()` — runs the synchronous poll
    /// loop on a `spawn_blocking` thread so it doesn't stall the
    /// tokio runtime. Most action-tool call sites should prefer this
    /// over the blocking `detect()`.
    pub async fn detect_async(self) -> Changes {
        // Move the Snapshot (and its embedded lease) onto the blocking
        // thread; the lease's Drop runs there when detect_with returns.
        tokio::task::spawn_blocking(move || self.detect())
            .await
            .unwrap_or_else(|_| Changes::no_change())
    }

    /// Detect changes using the action's latency budget.
    ///
    /// Codex Computer Use verifies the result with an explicit skyshot after
    /// one or more actions. Its compatibility actions therefore do not need
    /// the native profile's one-second no-change observation window. The
    /// private marker is injected only by the compatibility adapter; ordinary
    /// native callers retain the original suppression/detection contract.
    pub async fn detect_async_for_args(self, args: &Value) -> Changes {
        let fast = args
            .get("_codex_compat_fast_action")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let deadline = self.taken_at + DEFAULT_TIMEOUT;
        let session = self.session.clone();
        let observed = tokio::task::spawn_blocking(move || self.observe_now()).await;
        let mut changes = match observed {
            Ok((changes, guard)) => {
                // Codex-compat actions verify with an explicit screenshot, so
                // they end the guard now (the lease drops with `guard`). Native
                // actions keep it armed until the deadline or the next action,
                // without making this reply wait for it.
                if !fast {
                    arm_guard(session.clone(), guard, deadline);
                }
                changes
            }
            Err(_) => Changes::no_change(),
        };
        changes.late_new_windows = take_late_windows(&session);
        changes
    }

    /// One observation right after the action. Returns the changes seen now
    /// and the guard (lease + every window visible now) for later settling.
    fn observe_now(self) -> (Changes, PendingGuard) {
        let current: Vec<WindowInfo> = windows::visible_windows()
            .into_iter()
            .filter(|w| w.layer == 0)
            .collect();
        let (new_windows, _closed) = Self::diff(&self.window_ids, &current);
        let foreground_changed = match (self.front_pid, apps::frontmost_pid()) {
            (Some(orig), Some(cur)) => orig != cur,
            _ => false,
        };
        let guard = PendingGuard {
            window_ids: current.iter().map(|w| w.window_id).collect(),
            _lease: self._lease,
        };
        (
            Changes {
                new_windows,
                foreground_changed,
                late_new_windows: Vec::new(),
            },
            guard,
        )
    }

    /// Perform one immediate post-action observation and release suppression.
    pub async fn detect_async_fast(self) -> Changes {
        tokio::task::spawn_blocking(move || self.detect_with(Duration::ZERO, Duration::ZERO))
            .await
            .unwrap_or_else(|_| Changes::no_change())
    }

    /// Same as `detect()` but with configurable timing — exposed for
    /// tests / callers that want a tighter or looser poll window.
    pub fn detect_with(self, timeout: Duration, poll_interval: Duration) -> Changes {
        if timeout.is_zero() {
            return self.detect_once();
        }
        let deadline = Instant::now() + timeout;
        loop {
            std::thread::sleep(poll_interval);

            let current: Vec<WindowInfo> = windows::visible_windows()
                .into_iter()
                .filter(|w| w.layer == 0)
                .collect();
            let current_ids: HashSet<u32> = current.iter().map(|w| w.window_id).collect();

            let new_windows: Vec<WindowEvent> = current
                .iter()
                .filter(|w| !self.window_ids.contains(&w.window_id))
                .map(|w| WindowEvent {
                    window_id: w.window_id,
                    pid: w.pid,
                    app_name: w.app_name.clone(),
                    title: w.title.clone(),
                })
                .collect();
            // Diff the other direction too — keeps unit tests honest
            // even though Swift's result_suffix only uses opened windows.
            let _closed: Vec<u32> = self
                .window_ids
                .iter()
                .copied()
                .filter(|id| !current_ids.contains(id))
                .collect();

            let current_front = apps::frontmost_pid();
            let foreground_changed = match (self.front_pid, current_front) {
                (Some(orig), Some(cur)) => orig != cur,
                _ => false,
            };

            if !new_windows.is_empty() || foreground_changed {
                return Changes {
                    new_windows,
                    foreground_changed,
                    late_new_windows: Vec::new(),
                };
            }
            if Instant::now() >= deadline {
                return Changes::no_change();
            }
        }
    }

    fn detect_once(self) -> Changes {
        let current: Vec<WindowInfo> = windows::visible_windows()
            .into_iter()
            .filter(|w| w.layer == 0)
            .collect();
        let current_ids: HashSet<u32> = current.iter().map(|w| w.window_id).collect();
        let new_windows: Vec<WindowEvent> = current
            .iter()
            .filter(|w| !self.window_ids.contains(&w.window_id))
            .map(|w| WindowEvent {
                window_id: w.window_id,
                pid: w.pid,
                app_name: w.app_name.clone(),
                title: w.title.clone(),
            })
            .collect();
        let current_front = apps::frontmost_pid();
        let foreground_changed = match (self.front_pid, current_front) {
            (Some(orig), Some(cur)) => orig != cur,
            _ => false,
        };
        let _closed = self
            .window_ids
            .iter()
            .filter(|id| !current_ids.contains(id))
            .count();
        Changes {
            new_windows,
            foreground_changed,
            late_new_windows: Vec::new(),
        }
    }

    // ── Internal helpers — also used by unit tests via the `pub(super)`
    // path so the diff logic can be exercised without driving the live
    // window enumerator. ────────────────────────────────────────────

    /// Pure-function diff: given the snapshot's window-id set + a
    /// list of currently-visible windows, return the (opened, closed)
    /// classification.
    ///
    /// Used by the reply observation and by guard settling. The helper's own windows (the
    /// agent cursor and other overlays) are never reported as opened: the agent didn't open
    /// them and can't act on them.
    pub(crate) fn diff(
        snapshot_ids: &HashSet<u32>,
        current: &[WindowInfo],
    ) -> (Vec<WindowEvent>, Vec<u32>) {
        let own_pid = i32::try_from(std::process::id()).unwrap_or(-1);
        let current_ids: HashSet<u32> = current.iter().map(|w| w.window_id).collect();
        let opened: Vec<WindowEvent> = current
            .iter()
            .filter(|w| !snapshot_ids.contains(&w.window_id) && w.pid != own_pid)
            .map(|w| WindowEvent {
                window_id: w.window_id,
                pid: w.pid,
                app_name: w.app_name.clone(),
                title: w.title.clone(),
            })
            .collect();
        let closed: Vec<u32> = snapshot_ids
            .iter()
            .copied()
            .filter(|id| !current_ids.contains(id))
            .collect();
        (opened, closed)
    }
}

// ── Guard window settle slot ─────────────────────────────────────────────────

/// Holds the guard window of the most recent native action after its tool
/// result was returned. The guard keeps the wildcard focus-steal lease armed
/// and remembers the before-state, so windows the action opens late are
/// reported on the next tool result instead of making every action wait.
///
/// One slot per process: a new action flushes the previous guard (its late
/// windows are diffed before the new action fires), and the guard's one-shot
/// deadline task expires it only if no newer action replaced it.
pub(crate) struct SettleSlot<T> {
    generation: u64,
    pending: Option<T>,
}

impl<T> SettleSlot<T> {
    pub(crate) const fn new() -> Self {
        Self {
            generation: 0,
            pending: None,
        }
    }

    /// Stores `value` as the pending guard. Returns its generation and the
    /// guard it replaced, which the caller must settle now.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn arm(&mut self, value: T) -> (u64, Option<T>) {
        self.generation = self.generation.wrapping_add(1);
        (self.generation, self.pending.replace(value))
    }

    /// Takes the pending guard (a new action is about to fire).
    pub(crate) fn flush(&mut self) -> Option<T> {
        self.pending.take()
    }

    /// Takes the pending guard only if it is still `generation`.
    pub(crate) fn expire(&mut self, generation: u64) -> Option<T> {
        if generation == self.generation {
            self.pending.take()
        } else {
            None
        }
    }
}

/// One [`SettleSlot`] per agent session, so one agent's next action never
/// ends another agent's guard or takes its late windows.
pub(crate) struct SessionSettleSlots<T> {
    slots: std::collections::HashMap<String, SettleSlot<T>>,
    /// Generations come from one counter so a removed and re-created slot
    /// never reuses a generation a stale deadline task still holds.
    next_generation: u64,
}

impl<T> SessionSettleSlots<T> {
    pub(crate) fn new() -> Self {
        Self {
            slots: std::collections::HashMap::new(),
            next_generation: 0,
        }
    }

    pub(crate) fn arm(&mut self, session: &str, value: T) -> (u64, Option<T>) {
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        let slot = self
            .slots
            .entry(session.to_owned())
            .or_insert_with(SettleSlot::new);
        slot.generation = generation;
        (generation, slot.pending.replace(value))
    }

    pub(crate) fn flush(&mut self, session: &str) -> Option<T> {
        let taken = self.slots.get_mut(session)?.flush();
        self.slots.remove(session);
        taken
    }

    pub(crate) fn expire(&mut self, session: &str, generation: u64) -> Option<T> {
        let slot = self.slots.get_mut(session)?;
        let taken = slot.expire(generation);
        if taken.is_some() {
            self.slots.remove(session);
        }
        taken
    }

    /// Sessions with a pending guard (bounded by live agents).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }
}

/// The guard of the last native action: the wildcard focus-steal lease and
/// the windows visible when its reply was sent.
struct PendingGuard {
    window_ids: HashSet<u32>,
    _lease: Option<SuppressionLease>,
}

/// Late windows are reported once; cap them so a busy desktop cannot grow
/// the buffer between two tool calls.
const MAX_LATE_WINDOWS: usize = 32;

/// Late-window buffers are kept for at most this many sessions.
const MAX_LATE_SESSIONS: usize = 64;

static GUARDS: std::sync::LazyLock<std::sync::Mutex<SessionSettleSlots<PendingGuard>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(SessionSettleSlots::new()));
static LATE_WINDOWS: std::sync::LazyLock<std::sync::Mutex<LateWindows>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(LateWindows::default()));

/// Late windows per session, oldest-touched session evicted first.
#[derive(Default)]
struct LateWindows {
    by_session: std::collections::HashMap<String, (u64, Vec<WindowEvent>)>,
    touches: u64,
}

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Ends a guard: one diff against the windows visible at reply time, then the
/// lease drops (with `guard`).
fn settle(session: &str, guard: PendingGuard) {
    let current: Vec<WindowInfo> = windows::visible_windows()
        .into_iter()
        .filter(|w| w.layer == 0)
        .collect();
    let (opened, _closed) = Snapshot::diff(&guard.window_ids, &current);
    if opened.is_empty() {
        return;
    }
    let mut late = lock(&LATE_WINDOWS);
    late.touches += 1;
    let touch = late.touches;
    let entry = late
        .by_session
        .entry(session.to_owned())
        .or_insert_with(|| (touch, Vec::new()));
    entry.0 = touch;
    entry.1.extend(opened);
    let overflow = entry.1.len().saturating_sub(MAX_LATE_WINDOWS);
    entry.1.drain(..overflow);
    while late.by_session.len() > MAX_LATE_SESSIONS {
        let oldest = late
            .by_session
            .iter()
            .min_by_key(|(_, (touched, _))| *touched)
            .map(|(key, _)| key.clone());
        match oldest {
            Some(key) => {
                late.by_session.remove(&key);
            }
            None => break,
        }
    }
}

fn take_late_windows(session: &str) -> Vec<WindowEvent> {
    lock(&LATE_WINDOWS)
        .by_session
        .remove(session)
        .map(|(_, windows)| windows)
        .unwrap_or_default()
}

/// Keeps `guard` armed until `deadline` (a one-shot timer, no polling) unless
/// the next action settles it first.
fn arm_guard(session: String, guard: PendingGuard, deadline: Instant) {
    let (generation, replaced) = lock(&GUARDS).arm(&session, guard);
    if let Some(replaced) = replaced {
        settle(&session, replaced);
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        let expired = lock(&GUARDS).expire(&session, generation);
        if let Some(guard) = expired {
            settle(&session, guard);
        }
        return;
    };
    let remaining = deadline.saturating_duration_since(Instant::now());
    runtime.spawn(async move {
        tokio::time::sleep(remaining).await;
        let expired = lock(&GUARDS).expire(&session, generation);
        if let Some(guard) = expired {
            let _ = tokio::task::spawn_blocking(move || settle(&session, guard)).await;
        }
    });
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::WindowBounds;

    fn win(id: u32, pid: i32, app: &str, title: &str) -> WindowInfo {
        WindowInfo {
            window_id: id,
            pid,
            app_name: app.into(),
            title: title.into(),
            bounds: WindowBounds {
                x: 0.,
                y: 0.,
                width: 100.,
                height: 100.,
            },
            layer: 0,
            alpha: 1.0,
            z_index: 0,
            is_on_screen: true,
            on_current_space: None,
            space_ids: None,
        }
    }

    #[test]
    fn diff_finds_opened_window() {
        let snap: HashSet<u32> = [1, 2].into_iter().collect();
        let cur = vec![
            win(1, 100, "Safari", "Home"),
            win(2, 100, "Safari", "Tab2"),
            win(3, 101, "Mail", "Inbox"),
        ];
        let (opened, closed) = Snapshot::diff(&snap, &cur);
        assert_eq!(opened.len(), 1);
        assert_eq!(opened[0].window_id, 3);
        assert_eq!(opened[0].app_name, "Mail");
        assert_eq!(opened[0].title, "Inbox");
        assert!(closed.is_empty());
    }

    #[test]
    fn diff_leaves_out_the_helpers_own_windows() {
        let snap: HashSet<u32> = [1].into_iter().collect();
        let own = i32::try_from(std::process::id()).unwrap();
        let cur = vec![
            win(1, 100, "Safari", "Home"),
            win(2, own, "Brigadier Computer Use", ""),
            win(3, 101, "Mail", "Inbox"),
        ];
        let (opened, _) = Snapshot::diff(&snap, &cur);
        assert_eq!(
            opened.iter().map(|w| w.window_id).collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn diff_finds_closed_window() {
        let snap: HashSet<u32> = [1, 2, 3].into_iter().collect();
        let cur = vec![win(1, 100, "Safari", "Home")];
        let (opened, closed) = Snapshot::diff(&snap, &cur);
        assert!(opened.is_empty());
        assert_eq!(closed.len(), 2);
        let closed_set: HashSet<u32> = closed.into_iter().collect();
        assert!(closed_set.contains(&2));
        assert!(closed_set.contains(&3));
    }

    #[test]
    fn diff_no_change() {
        let snap: HashSet<u32> = [1, 2].into_iter().collect();
        let cur = vec![win(1, 100, "Safari", "A"), win(2, 100, "Safari", "B")];
        let (opened, closed) = Snapshot::diff(&snap, &cur);
        assert!(opened.is_empty());
        assert!(closed.is_empty());
    }

    #[test]
    fn changes_result_suffix_no_change_is_empty() {
        let c = Changes::no_change();
        assert_eq!(c.result_suffix(), "");
        assert!(!c.needs_restore());
    }

    #[test]
    fn changes_result_suffix_single_new_window_with_title() {
        let c = Changes {
            new_windows: vec![WindowEvent {
                window_id: 99,
                pid: 100,
                app_name: "Chrome".into(),
                title: "New Tab".into(),
            }],
            foreground_changed: false,
            late_new_windows: Vec::new(),
        };
        assert!(c.needs_restore());
        assert_eq!(
            c.result_suffix(),
            "\n\n🪟 Action opened new window(s): Chrome (\"New Tab\")."
        );
    }

    #[test]
    fn changes_result_suffix_groups_windows_by_app() {
        let c = Changes {
            new_windows: vec![
                WindowEvent {
                    window_id: 1,
                    pid: 100,
                    app_name: "Chrome".into(),
                    title: "Tab A".into(),
                },
                WindowEvent {
                    window_id: 2,
                    pid: 100,
                    app_name: "Chrome".into(),
                    title: "Tab B".into(),
                },
                WindowEvent {
                    window_id: 3,
                    pid: 101,
                    app_name: "Mail".into(),
                    title: "".into(),
                },
            ],
            foreground_changed: true,
            late_new_windows: Vec::new(),
        };
        let suffix = c.result_suffix();
        // BTreeMap sort order is alphabetical by app name → Chrome before Mail.
        assert_eq!(
            suffix,
            "\n\n🪟 Action opened new window(s): Chrome (\"Tab A\", \"Tab B\"); Mail."
        );
    }

    #[test]
    fn changes_result_suffix_foreground_change_only() {
        let c = Changes {
            new_windows: vec![],
            foreground_changed: true,
            late_new_windows: Vec::new(),
        };
        assert!(c.needs_restore());
        assert_eq!(
            c.result_suffix(),
            "\n\n🔀 Action caused a different app to become frontmost."
        );
    }

    #[test]
    fn changes_result_suffix_empty_title_is_dropped() {
        let c = Changes {
            new_windows: vec![WindowEvent {
                window_id: 1,
                pid: 100,
                app_name: "Finder".into(),
                title: "".into(),
            }],
            foreground_changed: false,
            late_new_windows: Vec::new(),
        };
        // No title → just the app name, no parentheses.
        assert_eq!(
            c.result_suffix(),
            "\n\n🪟 Action opened new window(s): Finder."
        );
    }

    /// Regression: `snapshot(prior_front)` must store the caller's
    /// captured front pid verbatim (rather than re-reading it inside
    /// the function and racing with concurrent activations).
    #[test]
    fn snapshot_stores_caller_prior_front() {
        // Use an obviously bogus pid so we'd notice if the impl silently
        // fell back to the live frontmost on this test runner.
        let bogus_prior = Some(424242_i32);
        let snap = WindowChangeDetector::snapshot(bogus_prior);
        assert_eq!(snap.front_pid(), bogus_prior);

        // None must round-trip too — and must skip the lease without
        // panicking (no frontmost to restore to).
        let snap_none = WindowChangeDetector::snapshot(None);
        assert_eq!(snap_none.front_pid(), None);
    }

    #[test]
    fn settle_slot_expires_only_its_own_generation() {
        let mut slot = SettleSlot::new();
        let (first, replaced) = slot.arm("a");
        assert_eq!(replaced, None);
        let (second, replaced) = slot.arm("b");
        assert_eq!(
            replaced,
            Some("a"),
            "arming hands back the guard it replaced"
        );
        assert_eq!(
            slot.expire(first),
            None,
            "a stale deadline must not take the newer guard"
        );
        assert_eq!(slot.expire(second), Some("b"));
        assert_eq!(slot.expire(second), None);
    }

    #[test]
    fn settle_slot_flush_takes_the_pending_guard() {
        let mut slot = SettleSlot::new();
        let (generation, _) = slot.arm(7);
        assert_eq!(slot.flush(), Some(7));
        assert_eq!(slot.flush(), None);
        assert_eq!(
            slot.expire(generation),
            None,
            "a flushed guard is settled exactly once"
        );
    }

    #[test]
    fn result_suffix_reports_windows_an_earlier_action_opened() {
        let mut c = Changes::no_change();
        c.late_new_windows.push(WindowEvent {
            window_id: 5,
            pid: 42,
            app_name: "Safari".into(),
            title: "Sign In".into(),
        });
        let suffix = c.result_suffix();
        assert!(suffix.contains("earlier action"), "{suffix}");
        assert!(suffix.contains("Safari (\"Sign In\")"), "{suffix}");
        assert!(
            !c.needs_restore(),
            "late windows were already guarded; no restore now"
        );
    }

    #[tokio::test]
    async fn native_detect_replies_without_waiting_for_the_guard_window() {
        let snapshot = WindowChangeDetector::snapshot(None);
        let started = Instant::now();
        let _ = snapshot.detect_async_for_args(&serde_json::json!({})).await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(300),
            "a native action reply must not wait for the 1 s guard window (took {elapsed:?})"
        );
    }

    #[test]
    fn one_session_never_settles_another_sessions_guard() {
        let mut slots = SessionSettleSlots::new();
        let (a_gen, _) = slots.arm("agent-a", "guard-a");
        let (b_gen, replaced) = slots.arm("agent-b", "guard-b");
        assert_eq!(
            replaced, None,
            "agent B's action must not replace agent A's guard"
        );
        assert_eq!(slots.flush("agent-b"), Some("guard-b"));
        assert_eq!(slots.expire("agent-b", b_gen), None);
        assert_eq!(
            slots.expire("agent-a", a_gen),
            Some("guard-a"),
            "A's deadline still ends A's guard"
        );
        assert_eq!(slots.len(), 0, "settled sessions leave no entry behind");
    }

    #[test]
    fn a_session_rearm_replaces_only_its_own_guard() {
        let mut slots = SessionSettleSlots::new();
        let (first, _) = slots.arm("agent-a", 1);
        let (_, replaced) = slots.arm("agent-a", 2);
        assert_eq!(replaced, Some(1));
        assert_eq!(
            slots.expire("agent-a", first),
            None,
            "a stale deadline must not end the newer guard"
        );
        assert_eq!(slots.flush("agent-a"), Some(2));
    }
}
