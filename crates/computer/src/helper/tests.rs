//! The helper's connections and engine queue over a fake desktop, through the daemon's own
//! client: no grants and no AppKit needed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::hub::{Access, Hub, System};
use crate::action::{ActRequest, Action, Expect, Screenshot};
use crate::cancel::{CancelToken, Held, InputGuard, Release};
use crate::client::{Answer, Client, Gone};
use crate::desktop::{
    AppInfo, Button, Capabilities, Capture, Chord, Desktop, Focus, Mods, UserFocus, WindowInfo,
};
use crate::error::{CuResult, ErrorCode, err};
use crate::geom::{ImageTransform, Point, Provider, Rect};
use crate::redact::Rgba;
use crate::tree::RawNode;
use crate::wire::{Event, Op, Permissions, Policy};

const TOKEN: &str = "the-token";
/// Long enough that a test finishing well inside it shows the wait was cut short.
const LONG_WAIT_MS: u64 = 20_000;
const SOON: Duration = Duration::from_secs(5);

/// What the test sees of the fake desktop, from any thread.
#[derive(Default)]
struct Seen {
    apps_calls: AtomicUsize,
    /// Told on every pump: an `act` is waiting.
    pumping: Mutex<Option<Sender<()>>>,
}

struct NoRelease;
impl Release for NoRelease {
    fn release(&self, _: Held) {}
}

/// One app with one window and nothing in it that ever changes.
struct Fake(Arc<Seen>);

fn window() -> WindowInfo {
    WindowInfo {
        id: 1,
        pid: 10,
        title: "Doc".into(),
        frame: Rect::new(0.0, 0.0, 200.0, 100.0),
        on_screen: true,
        minimized: false,
    }
}

impl Desktop for Fake {
    type Element = u32;
    fn releaser(&self) -> Arc<dyn Release + Send + Sync> {
        Arc::new(NoRelease)
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            structure: true,
            capture: true,
            element_actions: true,
            background_keys: true,
            background_pointer: true,
            synthetic_activation: false,
        }
    }
    fn apps(&mut self) -> CuResult<Vec<AppInfo>> {
        self.0.apps_calls.fetch_add(1, Ordering::SeqCst);
        let mut a = self.app(10)?;
        a.windows = vec![window()];
        Ok(vec![a])
    }
    fn windows(&mut self, _: i32) -> CuResult<Vec<WindowInfo>> {
        Ok(vec![window()])
    }
    fn window(&mut self, id: u32) -> CuResult<WindowInfo> {
        if id == 1 {
            Ok(window())
        } else {
            err(ErrorCode::NoSuchTarget, "no window")
        }
    }
    fn app(&mut self, pid: i32) -> CuResult<AppInfo> {
        Ok(AppInfo {
            pid,
            name: "Fake".into(),
            bundle_id: Some("dev.example.fake".into()),
            bundle_path: None,
            frontmost: false,
            windows: Vec::new(),
        })
    }
    fn tree(&mut self, _: &WindowInfo, _: bool) -> CuResult<Vec<RawNode<u32>>> {
        let mut n = RawNode::new(1, 0, "window");
        n.label = Some("Doc".into());
        n.frame = Some(Rect::new(0.0, 0.0, 200.0, 100.0));
        Ok(vec![n])
    }
    fn read(&mut self, w: &WindowInfo, _: &u32) -> CuResult<RawNode<u32>> {
        Ok(self.tree(w, false)?.remove(0))
    }
    fn backing_scale(&mut self, _: &WindowInfo) -> f64 {
        1.0
    }
    fn capture(&mut self, w: &WindowInfo, crop: Rect, ppp: f64, max: u32) -> CuResult<Capture> {
        let transform = ImageTransform::fit(w.id, w.frame, crop, ppp, max);
        let (width, height) = (transform.width, transform.height);
        Ok(Capture {
            image: Rgba {
                width,
                height,
                data: vec![255; (width * height * 4) as usize],
            },
            transform,
        })
    }
    fn perform(&mut self, _: &u32, _: &str) -> CuResult<()> {
        Ok(())
    }
    fn set_value(&mut self, _: &u32, _: &str) -> CuResult<()> {
        Ok(())
    }
    fn insert_text(&mut self, _: &u32, _: &str) -> CuResult<()> {
        Ok(())
    }
    fn set_focus(&mut self, _: &u32) -> CuResult<()> {
        Ok(())
    }
    fn select(&mut self, _: &u32, _: usize, _: usize) -> CuResult<()> {
        Ok(())
    }
    fn selection(&mut self, _: &u32) -> Option<(usize, usize)> {
        None
    }
    fn menu(&mut self, _: i32, _: &[String]) -> CuResult<()> {
        Ok(())
    }
    fn focus(&mut self, _: i32) -> CuResult<Focus<u32>> {
        Ok(Focus {
            element: None,
            window: Some(1),
            secure: false,
            role: None,
            selected_text: None,
        })
    }
    fn click(
        &mut self,
        _: &WindowInfo,
        _: Point,
        _: Button,
        _: u8,
        _: Mods,
        _: bool,
        _: &mut InputGuard<'_>,
    ) -> CuResult<()> {
        Ok(())
    }
    fn scroll(&mut self, _: &WindowInfo, _: Point, _: i32, _: i32) -> CuResult<()> {
        Ok(())
    }
    fn drag(
        &mut self,
        _: &WindowInfo,
        _: Point,
        _: Point,
        _: bool,
        _: &mut InputGuard<'_>,
        _: &CancelToken,
    ) -> CuResult<()> {
        Ok(())
    }
    fn key(&mut self, _: i32, _: &Chord, _: &mut InputGuard<'_>) -> CuResult<()> {
        Ok(())
    }
    fn type_text(&mut self, _: i32, _: &str, _: &CancelToken) -> CuResult<()> {
        Ok(())
    }
    fn watch(&mut self, _: i32) {}
    fn pump(&mut self, d: Duration) {
        if let Some(tx) = self.0.pumping.lock().unwrap().as_ref() {
            let _ = tx.send(());
        }
        std::thread::sleep(d);
    }
    fn last_notification(&self, _: i32) -> Option<Instant> {
        None
    }
    fn user_focus(&mut self) -> UserFocus {
        UserFocus {
            frontmost_pid: 99,
            frontmost_window: Some("The user's own window".into()),
            cursor: Point::new(5.0, 5.0),
            server_front: None,
        }
    }
}

fn granted() -> System {
    System {
        permissions: || Permissions {
            accessibility: true,
            screen_recording: true,
        },
        request_permission: |_| {},
        process_start_us: |_| Some(42),
    }
}

/// A hub on a socket in its own scratch folder, removed when the test ends.
struct Setup {
    hub: Arc<Hub>,
    socket: PathBuf,
    dir: PathBuf,
    seen: Arc<Seen>,
    /// One message per pump while an `act` waits.
    pumping: Receiver<()>,
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup(system: System, parent: Option<i32>) -> Setup {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "bcu-hub-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let socket = dir.join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    let seen = Arc::new(Seen::default());
    let (tx, pumping) = mpsc::channel();
    *seen.pumping.lock().unwrap() = Some(tx);
    let s = seen.clone();
    let access = Access {
        token: TOKEN.into(),
        parent,
        team: None,
    };
    let hub = Hub::start(access, system, move || Ok(Fake(s.clone()))).unwrap();
    hub.accept(listener).unwrap();
    Setup {
        hub,
        socket,
        dir,
        seen,
        pumping,
    }
}

fn connect(s: &Setup, token: &str) -> Client {
    Client::connect(&s.socket, token, |_| {}).unwrap()
}

type Answered = Receiver<Result<Answer, Gone>>;

fn send(c: &Client, op: Op) -> (u64, Answered) {
    send_as(c, "w1", op)
}

fn send_as(c: &Client, worker: &str, op: Op) -> (u64, Answered) {
    let id = c.next_id();
    let (tx, rx) = mpsc::channel();
    c.send(
        id,
        worker,
        Provider::Claude,
        Policy::default(),
        op,
        move |a| {
            let _ = tx.send(a);
        },
    );
    (id, rx)
}

fn answer(rx: &Answered) -> Result<Answer, Gone> {
    rx.recv_timeout(SOON).expect("an answer in time")
}

/// An `act` that waits for something that never appears, checking its token between polls.
fn long_act() -> Op {
    Op::Act(ActRequest {
        window: 1,
        actions: vec![Action::Wait {
            expect: Expect::Appears {
                find: "never there".into(),
            },
            timeout_ms: LONG_WAIT_MS,
        }],
        screenshot: Screenshot::Never,
    })
}

fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < SOON, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_wrong_token_or_a_stranger_is_refused() {
    let s = setup(granted(), None);
    let bad = connect(&s, "not-the-token");
    let (_, rx) = send(&bad, Op::Ping);
    assert!(answer(&rx).is_err(), "a wrong token gets no answer");
    let good = connect(&s, TOKEN);
    let (_, rx) = send(&good, Op::Ping);
    let a = answer(&rx).unwrap();
    assert!(a.reply.ok);
    assert_eq!(a.reply.text, "pong");

    // With a parent named, only that process gets in, token or not. This test process is the
    // peer here, so it passes as its own parent and fails as anyone else's.
    let me = std::process::id() as i32;
    let s = setup(granted(), Some(me));
    let c = connect(&s, TOKEN);
    let (_, rx) = send(&c, Op::Ping);
    assert!(answer(&rx).unwrap().reply.ok);
    let s = setup(granted(), Some(1));
    let c = connect(&s, TOKEN);
    let (_, rx) = send(&c, Op::Ping);
    assert!(
        answer(&rx).is_err(),
        "a peer that isn't the parent is refused"
    );
}

#[test]
fn control_is_answered_while_the_engine_is_busy_and_a_queued_cancel_ends_it_unstarted() {
    let s = setup(granted(), None);
    let c = connect(&s, TOKEN);
    let started = Instant::now();
    let (act, act_rx) = send(&c, long_act());
    s.pumping.recv_timeout(SOON).expect("the act is running");

    let (_, rx) = send(&c, Op::Ping);
    assert_eq!(answer(&rx).unwrap().reply.text, "pong");
    let (_, rx) = send(&c, Op::Permissions);
    let p = answer(&rx).unwrap().reply.permissions.unwrap();
    assert!(p.accessibility && p.screen_recording);

    // Queued behind the act, then cancelled: it ends without reaching the desktop.
    let (apps, apps_rx) = send(&c, Op::Apps);
    let (_, rx) = send(&c, Op::Cancel { request: apps });
    assert_eq!(answer(&rx).unwrap().reply.text, "cancelled");
    // Cancelling the running act ends its wait between two polls.
    let (_, rx) = send(&c, Op::Cancel { request: act });
    assert_eq!(answer(&rx).unwrap().reply.text, "cancelled");

    let a = answer(&act_rx).unwrap().reply;
    assert_eq!(
        a.results[0].error.as_ref().map(|e| e.code),
        Some(ErrorCode::Cancelled)
    );
    assert!(started.elapsed() < Duration::from_millis(LONG_WAIT_MS / 2));
    let q = answer(&apps_rx).unwrap().reply;
    assert!(!q.ok);
    let e = q.error.unwrap();
    assert_eq!(e.code, ErrorCode::Cancelled);
    assert_eq!(e.detail, "the request was cancelled");
    assert_eq!(s.seen.apps_calls.load(Ordering::SeqCst), 0);

    // Later requests run normally, and a cancel of a finished one is harmless.
    let (_, rx) = send(&c, Op::Apps);
    let r = answer(&rx).unwrap().reply;
    assert!(r.ok, "{r:?}");
    assert!(r.text.contains("Fake pid 10"));
    assert!(r.engine_ms >= 0.0);
    let (_, rx) = send(&c, Op::Cancel { request: apps });
    assert!(answer(&rx).unwrap().reply.ok);
}

#[test]
fn a_dropped_connection_cancels_its_running_and_queued_requests_only() {
    let s = setup(granted(), None);
    let first = connect(&s, TOKEN);
    let second = connect(&s, TOKEN);
    let (_, rx) = send(&second, Op::Ping);
    answer(&rx).unwrap();
    let started = Instant::now();
    let (_, _act) = send(&first, long_act());
    s.pumping.recv_timeout(SOON).expect("the act is running");
    let (_, _queued) = send(&first, Op::Apps);
    wait_until("both connections are in", || s.hub.connections() == 2);
    drop(first);
    wait_until("the first connection is gone", || s.hub.connections() == 1);

    // The other connection's request runs at once: the act's wait was cut short, and the
    // dropped connection's queued `apps` never reached the desktop.
    let (_, rx) = send(&second, Op::Apps);
    let r = answer(&rx).unwrap().reply;
    assert!(r.ok, "{r:?}");
    assert!(started.elapsed() < Duration::from_millis(LONG_WAIT_MS / 2));
    assert_eq!(s.seen.apps_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn the_users_stop_ends_running_work_and_is_told_to_every_connection() {
    let s = setup(granted(), None);
    let (tx, events) = mpsc::channel::<Event>();
    let tx = Mutex::new(tx);
    let c = Client::connect(&s.socket, TOKEN, move |e| {
        let _ = tx.lock().unwrap().send(e);
    })
    .unwrap();
    let (_, act_rx) = send(&c, long_act());
    s.pumping.recv_timeout(SOON).expect("the act is running");
    s.hub.stop("hotkey");
    assert_eq!(
        events.recv_timeout(SOON).unwrap(),
        Event::Stopped {
            by: "hotkey".into()
        }
    );
    let a = answer(&act_rx).unwrap().reply;
    assert_eq!(
        a.results[0].error.as_ref().map(|e| e.code),
        Some(ErrorCode::StoppedByUser)
    );
    // A request after the stop runs normally.
    let (_, rx) = send(&c, Op::Apps);
    assert!(answer(&rx).unwrap().reply.ok);
}

#[test]
fn a_missing_grant_is_named_and_the_control_service_still_answers() {
    let system = System {
        permissions: || Permissions {
            accessibility: false,
            screen_recording: true,
        },
        ..granted()
    };
    let s = setup(system, None);
    let c = connect(&s, TOKEN);
    let (_, rx) = send(&c, Op::Apps);
    let r = answer(&rx).unwrap().reply;
    let e = r.error.unwrap();
    assert_eq!(e.code, ErrorCode::PermissionMissing);
    assert_eq!(
        e.detail,
        "Brigadier Computer Use isn't allowed to control apps (Accessibility)"
    );
    assert_eq!(s.seen.apps_calls.load(Ordering::SeqCst), 0);
    let (_, rx) = send(&c, Op::Permissions);
    let p = answer(&rx).unwrap().reply.permissions.unwrap();
    assert!(!p.accessibility && p.screen_recording);
}

#[test]
fn describe_names_the_instance_and_sessions_decide_idleness() {
    let s = setup(granted(), None);
    let c = connect(&s, TOKEN);
    assert!(s.hub.idle_for().is_some(), "idle before any session");
    let (_, rx) = send_as(&c, "w2", Op::Describe { window: 1 });
    let r = answer(&rx).unwrap().reply;
    let d = r.described.unwrap();
    assert_eq!((d.instance.pid, d.instance.started_us), (10, 42));
    assert_eq!(d.app_name, "Fake");
    assert_eq!(d.blocked, None);
    // The session stays open after its request until the worker ends it.
    assert!(s.hub.idle_for().is_none(), "a session is open");
    let (_, rx) = send_as(&c, "w2", Op::EndSession);
    assert!(answer(&rx).unwrap().reply.ok);
    assert!(
        s.hub.idle_for().is_some(),
        "idle once the last session ended"
    );
    let (_, rx) = send(&c, Op::Launch(Default::default()));
    let e = answer(&rx).unwrap().reply.error.unwrap();
    assert_eq!(e.code, ErrorCode::BadRequest);
    // A backend that can't open apps says so.
    let (_, rx) = send(
        &c,
        Op::Launch(crate::wire::LaunchRequest {
            app: Some("Notes".into()),
            open: None,
        }),
    );
    let e = answer(&rx).unwrap().reply.error.unwrap();
    assert_eq!(e.code, ErrorCode::UnsupportedCapability);
}
