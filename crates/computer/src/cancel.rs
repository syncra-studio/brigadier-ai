//! Cancellation generations, deadlines and the input-release guard (§4.7).
//!
//! Stop, a cancelled request, a worker that ends or a closed connection bumps a generation.
//! Every action checks its generation before it starts, and long sequences check it between
//! events. Whatever a request pressed is released on every way out, a panic included.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::error::{CuError, CuResult, ErrorCode};

/// The generations requests are checked against: one global, one per session.
#[derive(Debug, Default)]
pub struct Generations {
    global: AtomicU64,
    sessions: Mutex<HashMap<String, u64>>,
}

impl Generations {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The global stop: every running request ends between two events.
    pub fn stop_all(&self) {
        self.global.fetch_add(1, Ordering::SeqCst);
    }

    /// Ends one session's running requests.
    pub fn cancel_session(&self, session: &str) {
        let mut map = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        *map.entry(session.to_owned()).or_default() += 1;
    }

    fn session(&self, session: &str) -> u64 {
        let map = self.sessions.lock().unwrap_or_else(|p| p.into_inner());
        map.get(session).copied().unwrap_or(0)
    }

    /// A token for a request that starts now. Requests started after a stop run normally.
    pub fn token(self: &Arc<Self>, session: &str, deadline: Duration) -> CancelToken {
        CancelToken {
            gens: Arc::clone(self),
            session: session.to_owned(),
            global_at_start: self.global.load(Ordering::SeqCst),
            session_at_start: self.session(session),
            deadline: Instant::now() + deadline,
        }
    }
}

/// One request's view of the generations, plus its deadline.
#[derive(Debug, Clone)]
pub struct CancelToken {
    gens: Arc<Generations>,
    session: String,
    global_at_start: u64,
    session_at_start: u64,
    deadline: Instant,
}

impl CancelToken {
    /// Fails when the user stopped, the session was cancelled or the deadline passed.
    pub fn check(&self) -> CuResult<()> {
        if self.gens.global.load(Ordering::SeqCst) != self.global_at_start {
            return Err(CuError::new(ErrorCode::StoppedByUser, "stopped"));
        }
        if self.gens.session(&self.session) != self.session_at_start {
            return Err(CuError::new(
                ErrorCode::Cancelled,
                "the session's requests were cancelled",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(CuError::new(
                ErrorCode::Deadline,
                "the request's deadline passed",
            ));
        }
        Ok(())
    }

    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// What a backend can release: a mouse button held in a window, or a modifier key held in an
/// app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Held {
    Button { pid: i32, window: u32, button: u8 },
    Key { pid: i32, keycode: u16 },
}

/// Releases held input. Backends implement it; the guard calls it.
pub trait Release {
    fn release(&self, held: Held);
}

/// Tracks what a request pressed and releases whatever is still down when it is dropped,
/// on success, error, cancellation or panic alike.
pub struct InputGuard<'a> {
    releaser: &'a dyn Release,
    held: Vec<Held>,
}

impl<'a> InputGuard<'a> {
    pub fn new(releaser: &'a dyn Release) -> Self {
        Self {
            releaser,
            held: Vec::new(),
        }
    }

    pub fn pressed(&mut self, h: Held) {
        self.held.push(h);
    }

    pub fn released(&mut self, h: Held) {
        if let Some(i) = self.held.iter().rposition(|x| *x == h) {
            self.held.remove(i);
        }
    }

    pub fn held(&self) -> &[Held] {
        &self.held
    }
}

impl Drop for InputGuard<'_> {
    fn drop(&mut self) {
        // Most recent first: a modifier pressed before a click is released after it.
        while let Some(h) = self.held.pop() {
            self.releaser.release(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Log(RefCell<Vec<Held>>);
    impl Release for Log {
        fn release(&self, h: Held) {
            self.0.borrow_mut().push(h);
        }
    }

    #[test]
    fn stop_ends_running_requests_but_not_later_ones() {
        let g = Generations::new();
        let t = g.token("s1", Duration::from_secs(30));
        assert!(t.check().is_ok());
        g.stop_all();
        assert_eq!(t.check().unwrap_err().code, ErrorCode::StoppedByUser);
        let later = g.token("s1", Duration::from_secs(30));
        assert!(later.check().is_ok());
    }

    #[test]
    fn a_session_cancel_spares_other_sessions() {
        let g = Generations::new();
        let a = g.token("a", Duration::from_secs(30));
        let b = g.token("b", Duration::from_secs(30));
        g.cancel_session("a");
        assert_eq!(a.check().unwrap_err().code, ErrorCode::Cancelled);
        assert!(b.check().is_ok());
    }

    #[test]
    fn a_passed_deadline_fails_the_check() {
        let g = Generations::new();
        let t = g.token("s", Duration::ZERO);
        assert_eq!(t.check().unwrap_err().code, ErrorCode::Deadline);
    }

    #[test]
    fn the_guard_releases_what_is_still_held_even_on_panic() {
        let log = Log::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut g = InputGuard::new(&log);
            g.pressed(Held::Key {
                pid: 1,
                keycode: 56,
            });
            g.pressed(Held::Button {
                pid: 1,
                window: 9,
                button: 0,
            });
            g.pressed(Held::Key {
                pid: 1,
                keycode: 55,
            });
            g.released(Held::Key {
                pid: 1,
                keycode: 55,
            });
            panic!("mid-drag");
        }));
        assert!(result.is_err());
        assert_eq!(
            *log.0.borrow(),
            vec![
                Held::Button {
                    pid: 1,
                    window: 9,
                    button: 0
                },
                Held::Key {
                    pid: 1,
                    keycode: 56
                }
            ]
        );
    }
}
