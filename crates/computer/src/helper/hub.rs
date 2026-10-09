//! The helper's connections and its engine thread, over any [`Desktop`] so tests can run it
//! without grants or AppKit (§4.2, §4.7).
//!
//! Each connection has a reader thread. It answers control requests itself and queues engine
//! requests for the one engine thread, which writes their replies back under the connection's
//! writer lock. Every engine request takes its cancellation token when it arrives, so a stop
//! or a cancel that comes while it waits ends it before it starts.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::action::{ImageOut, Reply as EngineReply};
use crate::block::BlockList;
use crate::cancel::{CancelToken, Generations};
use crate::desktop::{Desktop, WindowInfo};
use crate::engine::{Engine, REQUEST_DEADLINE};
use crate::error::{CuError, CuResult, ErrorCode};
use crate::geom::Provider;
use crate::wire::{
    Described, Event, Grant, Hello, HelperFrame, ImageMeta, Instance, Launched, Op, PROTOCOL,
    Permissions, Reply, Request, read_frame, write_frame,
};

/// How long a new connection has to send its hello.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// What the control service and the engine thread ask of the system. Plain functions, so tests
/// can stand in for the system's answers.
#[derive(Clone, Copy)]
pub struct System {
    pub permissions: fn() -> Permissions,
    /// Shows the system's own prompt for a grant; returns at once.
    pub request_permission: fn(Grant),
    /// A process's start time, microseconds since the Unix epoch.
    pub process_start_us: fn(i32) -> Option<u64>,
}

/// Who may connect.
#[derive(Debug, Clone)]
pub struct Access {
    /// The per-launch token every connection's hello must carry.
    pub token: String,
    /// The daemon that started the helper: when set, only its connections are taken.
    pub parent: Option<i32>,
    /// This helper's signing team: when set, the peer must be signed by the same team.
    pub team: Option<String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Who is using the helper, for the idle exit.
struct Activity {
    /// Workers that sent engine work and haven't ended their session.
    sessions: HashSet<String>,
    /// Engine requests queued or running.
    running: usize,
    /// Since when nothing has been in use.
    idle_since: Option<Instant>,
}

impl Activity {
    fn start(&mut self, worker: &str) {
        self.sessions.insert(worker.to_owned());
        self.running += 1;
        self.idle_since = None;
    }

    fn done(&mut self) {
        self.running = self.running.saturating_sub(1);
        self.mark_idle();
    }

    fn end(&mut self, worker: &str) {
        self.sessions.remove(worker);
        self.mark_idle();
    }

    fn mark_idle(&mut self) {
        if self.sessions.is_empty() && self.running == 0 && self.idle_since.is_none() {
            self.idle_since = Some(Instant::now());
        }
    }
}

/// One connection's writing side and its engine requests in flight.
struct Conn {
    writer: Mutex<UnixStream>,
    /// Engine requests queued or running: the request's id on the wire, and the key its token
    /// was taken with (unique across connections).
    live: Mutex<HashMap<u64, u64>>,
}

impl Conn {
    /// Writes a frame and the images after it as one unit, so frames from the reader, the
    /// engine thread and a stop never interleave. A failed write is left to the reader, which
    /// sees the connection end.
    fn send(&self, frame: &HelperFrame, images: &[&[u8]]) {
        let Ok(body) = serde_json::to_vec(frame) else {
            return;
        };
        let mut w = lock(&self.writer);
        if write_frame(&mut *w, &body).is_err() {
            return;
        }
        for img in images {
            if write_frame(&mut *w, img).is_err() {
                return;
            }
        }
    }

    fn reply(&self, reply: Reply, images: &[&[u8]]) {
        self.send(&HelperFrame::Reply(Box::new(reply)), images);
    }
}

/// An engine request on its way to the engine thread.
struct Job {
    conn: Arc<Conn>,
    req: Request,
    key: u64,
    /// Taken when the request arrived.
    token: CancelToken,
}

enum Work {
    Job(Box<Job>),
    /// A worker ended: drop what the engine keeps for it.
    Forget(String),
}

/// The helper's shared state: connections, the engine queue, generations and activity.
pub struct Hub {
    access: Access,
    system: System,
    pub gens: Arc<Generations>,
    jobs: Sender<Work>,
    conns: Mutex<HashMap<u64, Arc<Conn>>>,
    activity: Arc<Mutex<Activity>>,
    next_conn: AtomicU64,
    next_key: AtomicU64,
}

impl Hub {
    /// Starts the engine thread. `make` builds the desktop there on the first engine request
    /// that finds every grant in place, and again after a failure.
    pub fn start<D, F>(access: Access, system: System, make: F) -> std::io::Result<Arc<Self>>
    where
        D: Desktop + 'static,
        F: FnMut() -> CuResult<D> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let gens = Generations::new();
        let activity = Arc::new(Mutex::new(Activity {
            sessions: HashSet::new(),
            running: 0,
            idle_since: Some(Instant::now()),
        }));
        let (g, a) = (gens.clone(), activity.clone());
        std::thread::Builder::new()
            .name("computer-engine".into())
            .spawn(move || engine_loop(rx, g, a, system, make))?;
        Ok(Arc::new(Self {
            access,
            system,
            gens,
            jobs: tx,
            conns: Mutex::default(),
            activity,
            next_conn: AtomicU64::new(1),
            next_key: AtomicU64::new(1),
        }))
    }

    /// Takes connections on `listener` until it fails, each on its own reader thread.
    pub fn accept(self: &Arc<Self>, listener: UnixListener) -> std::io::Result<()> {
        let hub = self.clone();
        std::thread::Builder::new()
            .name("computer-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let hub = hub.clone();
                    let _ = std::thread::Builder::new()
                        .name("computer-conn".into())
                        .spawn(move || hub.serve(stream));
                }
            })?;
        Ok(())
    }

    /// The user's stop from the menu or the hotkey: every running and queued request ends,
    /// and every connection hears it. Never waits: the events go out from their own thread.
    pub fn stop(self: &Arc<Self>, by: &str) {
        self.gens.stop_all();
        let hub = self.clone();
        let by = by.to_owned();
        let _ = std::thread::Builder::new()
            .name("computer-stop".into())
            .spawn(move || {
                let conns: Vec<Arc<Conn>> = lock(&hub.conns).values().cloned().collect();
                let event = HelperFrame::Event(Event::Stopped { by });
                for c in conns {
                    c.send(&event, &[]);
                }
            });
    }

    /// How long nothing has been in use: no session open and no request queued or running.
    pub fn idle_for(&self) -> Option<Duration> {
        lock(&self.activity).idle_since.map(|t| t.elapsed())
    }

    /// Connections that passed the hello and haven't ended.
    pub fn connections(&self) -> usize {
        lock(&self.conns).len()
    }

    fn admit(&self, stream: &mut UnixStream) -> Result<(), String> {
        super::peer::check(stream, self.access.parent, self.access.team.as_deref())?;
        stream
            .set_read_timeout(Some(HELLO_TIMEOUT))
            .map_err(|e| e.to_string())?;
        let frame = read_frame(stream).map_err(|e| format!("no hello: {e}"))?;
        let hello: Hello = serde_json::from_slice(&frame).map_err(|e| format!("bad hello: {e}"))?;
        if hello.protocol != PROTOCOL {
            return Err(format!(
                "protocol {} (this helper speaks {PROTOCOL})",
                hello.protocol
            ));
        }
        if !same_secret(&hello.token, &self.access.token) {
            return Err("wrong token".into());
        }
        stream.set_read_timeout(None).map_err(|e| e.to_string())
    }

    /// One connection, on its reader thread.
    fn serve(self: Arc<Self>, mut stream: UnixStream) {
        if let Err(why) = self.admit(&mut stream) {
            eprintln!("brigadier-computer: refused a connection: {why}");
            let _ = stream.shutdown(std::net::Shutdown::Both);
            return;
        }
        let Ok(writer) = stream.try_clone() else {
            return;
        };
        let conn = Arc::new(Conn {
            writer: Mutex::new(writer),
            live: Mutex::default(),
        });
        let id = self.next_conn.fetch_add(1, Ordering::SeqCst);
        lock(&self.conns).insert(id, conn.clone());
        while let Ok(frame) = read_frame(&mut stream) {
            match serde_json::from_slice::<Request>(&frame) {
                Ok(req) if req.op.is_control() => {
                    let reply = self.control(&conn, &req);
                    conn.reply(reply, &[]);
                }
                Ok(req) => self.queue(&conn, req),
                Err(e) => {
                    // Answer it when its id can be read; otherwise the stream can't be trusted.
                    let id = serde_json::from_slice::<serde_json::Value>(&frame)
                        .ok()
                        .and_then(|v| v.get("id").and_then(serde_json::Value::as_u64));
                    let Some(id) = id else { break };
                    conn.reply(
                        Reply::error(id, CuError::new(ErrorCode::BadRequest, e.to_string())),
                        &[],
                    );
                }
            }
        }
        // The connection ended: nobody is left to read its replies, so its requests end too.
        lock(&self.conns).remove(&id);
        for key in lock(&conn.live).values() {
            self.gens.cancel_request(*key);
        }
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }

    fn queue(&self, conn: &Arc<Conn>, req: Request) {
        let key = self.next_key.fetch_add(1, Ordering::SeqCst);
        {
            let mut live = lock(&conn.live);
            if live.contains_key(&req.id) {
                drop(live);
                let e = CuError::new(
                    ErrorCode::BadRequest,
                    format!("request {} is already queued or running", req.id),
                );
                conn.reply(Reply::error(req.id, e), &[]);
                return;
            }
            live.insert(req.id, key);
        }
        // Its deadline is set again when it starts.
        let token = self.gens.request_token(&req.worker, key, Duration::ZERO);
        lock(&self.activity).start(&req.worker);
        let job = Box::new(Job {
            conn: conn.clone(),
            req,
            key,
            token,
        });
        if let Err(mpsc::SendError(Work::Job(job))) = self.jobs.send(Work::Job(job)) {
            let e = CuError::new(ErrorCode::Failed, "the engine thread is gone");
            finish(
                &self.gens,
                &self.activity,
                &job,
                Reply::error(job.req.id, e),
                &[],
            );
        }
    }

    fn control(&self, conn: &Conn, req: &Request) -> Reply {
        let ok = |text: &str| Reply {
            id: req.id,
            ok: true,
            text: text.to_owned(),
            ..Default::default()
        };
        match &req.op {
            Op::Permissions => self.permissions(req.id),
            Op::RequestPermission { grant } => {
                (self.system.request_permission)(*grant);
                self.permissions(req.id)
            }
            Op::Cancel { request } => {
                // Under the live lock, so the request can't finish between the lookup and the
                // cancel and leave its id behind.
                let live = lock(&conn.live);
                match live.get(request) {
                    Some(key) => {
                        self.gens.cancel_request(*key);
                        ok("cancelled")
                    }
                    None => ok("no such request is queued or running"),
                }
            }
            Op::EndSession => {
                self.gens.cancel_session(&req.worker);
                lock(&self.activity).end(&req.worker);
                let _ = self.jobs.send(Work::Forget(req.worker.clone()));
                ok("session ended")
            }
            Op::StopAll => {
                self.gens.stop_all();
                ok("stopped")
            }
            Op::Ping => ok("pong"),
            _ => Reply::error(
                req.id,
                CuError::new(ErrorCode::BadRequest, "not a control request"),
            ),
        }
    }

    fn permissions(&self, id: u64) -> Reply {
        let p = (self.system.permissions)();
        let word = |b: bool| if b { "allowed" } else { "not allowed" };
        Reply {
            id,
            ok: true,
            text: format!(
                "accessibility: {} · screen recording: {}",
                word(p.accessibility),
                word(p.screen_recording)
            ),
            permissions: Some(p),
            ..Default::default()
        }
    }
}

/// Compares two secrets in time that doesn't depend on where they differ.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// What is missing, in the words the user sees, or `None` when both grants are in place.
pub fn missing(p: Permissions) -> Option<CuError> {
    let mut what = Vec::new();
    if !p.accessibility {
        what.push("to control apps (Accessibility)");
    }
    if !p.screen_recording {
        what.push("to see the screen (Screen Recording)");
    }
    (!what.is_empty()).then(|| {
        CuError::new(
            ErrorCode::PermissionMissing,
            format!("Brigadier Computer Use isn't allowed {}", what.join(" or ")),
        )
    })
}

/// Ends a request: forgets its id, writes its reply and counts it done.
fn finish(
    gens: &Generations,
    activity: &Mutex<Activity>,
    job: &Job,
    reply: Reply,
    images: &[&[u8]],
) {
    {
        let mut live = lock(&job.conn.live);
        live.remove(&job.req.id);
        gens.finish_request(job.key);
    }
    job.conn.reply(reply, images);
    lock(activity).done();
}

fn engine_loop<D, F>(
    rx: Receiver<Work>,
    gens: Arc<Generations>,
    activity: Arc<Mutex<Activity>>,
    system: System,
    mut make: F,
) where
    D: Desktop,
    F: FnMut() -> CuResult<D>,
{
    let mut engine: Option<Engine<D>> = None;
    for work in rx {
        match work {
            Work::Forget(worker) => {
                if let Some(e) = engine.as_mut() {
                    e.forget_worker(&worker);
                }
            }
            Work::Job(job) => {
                let started = Instant::now();
                let run = catch_unwind(AssertUnwindSafe(|| {
                    run(&mut engine, &mut make, &gens, system, &job)
                }));
                let (mut reply, images) = match run {
                    Ok(Ok((reply, images))) => (reply, images),
                    Ok(Err(e)) => (Reply::error(job.req.id, e), Vec::new()),
                    Err(_) => {
                        // Its state can't be trusted after a panic; the next request makes a
                        // new one.
                        engine = None;
                        let e =
                            CuError::new(ErrorCode::Failed, "the engine failed on this request");
                        (Reply::error(job.req.id, e), Vec::new())
                    }
                };
                reply.engine_ms = (started.elapsed().as_secs_f64() * 1e5).round() / 100.0;
                let images: Vec<&[u8]> = images.iter().map(Vec::as_slice).collect();
                finish(&gens, &activity, &job, reply, &images);
            }
        }
    }
}

/// Runs one engine request: its reply, and the images that follow it.
fn run<D: Desktop>(
    slot: &mut Option<Engine<D>>,
    make: &mut impl FnMut() -> CuResult<D>,
    gens: &Arc<Generations>,
    system: System,
    job: &Job,
) -> CuResult<(Reply, Vec<Vec<u8>>)> {
    let req = &job.req;
    // Cancelled or stopped while it waited: it never starts.
    job.token.with_deadline(REQUEST_DEADLINE).check()?;
    if let Some(e) = missing((system.permissions)()) {
        return Err(e);
    }
    let engine = match slot {
        Some(e) => e,
        None => {
            let mut e = Engine::new(make()?, BlockList::default(), Provider::Claude);
            e.gens = gens.clone();
            slot.insert(e)
        }
    };
    engine.block = req.policy.block_list();
    engine.provider = req.provider;
    let reply = |text: String| Reply {
        id: req.id,
        ok: true,
        text,
        ..Default::default()
    };
    let out = match &req.op {
        Op::Apps => (reply(engine.apps_text()?), Vec::new()),
        Op::Observe(r) => from_engine(req.id, engine.observe(&req.worker, r)?),
        Op::Act(r) => {
            let done = engine.act_from(&req.worker, r, Some(&job.token));
            let records = std::mem::take(&mut engine.records);
            let (mut reply, images) = from_engine(req.id, done?);
            reply.records = records;
            (reply, images)
        }
        Op::Zoom(r) => from_engine(req.id, engine.zoom(&req.worker, r)?),
        Op::Describe { window } => {
            let described = describe(engine, system, *window)?;
            let mut r = reply(format!(
                "w{} {:?} · {} pid {}{}\n",
                described.window.id,
                described.window.title,
                described.app_name,
                described.instance.pid,
                described
                    .blocked
                    .as_deref()
                    .map(|b| format!(" · blocked ({b})"))
                    .unwrap_or_default()
            ));
            r.described = Some(described);
            (r, Vec::new())
        }
        Op::CloseWindows { instance, windows } => (
            reply(close_windows(engine, system, instance, windows)),
            Vec::new(),
        ),
        Op::Launch(l) => {
            let o = crate::launch::launch(engine, l, &job.token.with_deadline(REQUEST_DEADLINE))?;
            let started_us = (system.process_start_us)(o.app.pid).ok_or_else(|| {
                CuError::new(ErrorCode::NoSuchTarget, "the app quit as it opened")
            })?;
            let mut text = format!(
                "{} pid {} · {}",
                o.app.name,
                o.app.pid,
                if o.new_process {
                    "started"
                } else {
                    "was already running"
                }
            );
            if o.new_windows.is_empty() {
                text.push_str(" · no new window");
            } else {
                let ids: Vec<String> = o.new_windows.iter().map(|w| format!("w{w}")).collect();
                let _ = write!(text, " · new window {}", ids.join(", "));
            }
            if !o.restored_windows.is_empty() {
                let ids: Vec<String> = o.restored_windows.iter().map(|w| format!("w{w}")).collect();
                let _ = write!(
                    text,
                    " · it also reopened the user's earlier {} {}, not yours",
                    if ids.len() == 1 { "window" } else { "windows" },
                    ids.join(", ")
                );
            }
            if o.front_restored {
                text.push_str(" · it took the front, which was given back");
            }
            text.push('\n');
            let mut r = reply(text);
            r.launched = Some(Launched {
                instance: Instance {
                    pid: o.app.pid,
                    started_us,
                },
                app_name: o.app.name,
                bundle_id: o.app.bundle_id,
                new_process: o.new_process,
                new_windows: o.new_windows,
                restored_windows: o.restored_windows,
                front_restored: o.front_restored,
            });
            (r, Vec::new())
        }
        _ => {
            return Err(CuError::new(
                ErrorCode::BadRequest,
                "a control request reached the engine",
            ));
        }
    };
    // Records belong to the request that made them; none may reach a later one.
    engine.records.clear();
    Ok(out)
}

/// How long closed windows get to go before `CloseWindows` reports the ones still open.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Closes the windows of `instance` still open, if it's still that process, and says which
/// stayed (an app asking about unsaved changes keeps its window).
fn close_windows<D: Desktop>(
    engine: &mut Engine<D>,
    system: System,
    instance: &Instance,
    windows: &[u32],
) -> String {
    if (system.process_start_us)(instance.pid) != Some(instance.started_us) {
        return "the app has already quit".into();
    }
    let open = |e: &mut Engine<D>| -> Vec<WindowInfo> {
        e.desktop
            .windows(instance.pid)
            .unwrap_or_default()
            .into_iter()
            .filter(|w| windows.contains(&w.id))
            .collect()
    };
    // A window that has just opened may miss the first press: the ones still open are
    // pressed again every half second. One asking about unsaved changes stays.
    let deadline = Instant::now() + CLOSE_WAIT;
    let mut left = open(engine);
    let mut pressed: Option<Instant> = None;
    while !left.is_empty() && Instant::now() < deadline {
        if pressed.is_none_or(|t| t.elapsed() >= Duration::from_millis(500)) {
            for w in &left {
                let _ = engine.desktop.close(w);
            }
            pressed = Some(Instant::now());
        }
        std::thread::sleep(Duration::from_millis(50));
        left = open(engine);
    }
    if left.is_empty() {
        "closed".into()
    } else {
        let ids: Vec<String> = left.iter().map(|w| format!("w{}", w.id)).collect();
        format!("still open: {}", ids.join(", "))
    }
}

fn from_engine(id: u64, r: EngineReply) -> (Reply, Vec<Vec<u8>>) {
    let mut pngs = Vec::new();
    let mut meta = |img: Option<ImageOut>| {
        img.map(
            |ImageOut {
                 id,
                 png,
                 width,
                 height,
                 tokens,
                 ..
             }| {
                let m = ImageMeta {
                    id,
                    width,
                    height,
                    tokens,
                    bytes: png.len(),
                    mime: "image/png".into(),
                };
                pngs.push(png);
                m
            },
        )
    };
    // The model's image first, then the log's: the order the frames follow the reply.
    let image = meta(r.image);
    let trajectory = meta(r.trajectory);
    let reply = Reply {
        id,
        ok: true,
        text: r.text,
        results: r.results,
        image,
        trajectory,
        ..Default::default()
    };
    (reply, pngs)
}

fn describe<D: Desktop>(
    engine: &mut Engine<D>,
    system: System,
    window: u32,
) -> CuResult<Described> {
    let w = engine.desktop.window(window)?;
    let app = engine.desktop.app(w.pid)?;
    let blocked = engine.block_reason(&w)?.map(str::to_owned);
    let started_us = (system.process_start_us)(w.pid).ok_or_else(|| {
        CuError::new(
            ErrorCode::NoSuchTarget,
            format!("the process behind w{window} is gone"),
        )
    })?;
    Ok(Described {
        instance: Instance {
            pid: w.pid,
            started_us,
        },
        window: w,
        app_name: app.name,
        bundle_id: app.bundle_id,
        bundle_path: app.bundle_path,
        blocked,
    })
}
