//! The broker's end of the helper socket (§4.2): one connection, requests multiplexed by id.
//!
//! No async runtime: a reader thread hands each reply to the callback its request left, so
//! the daemon can turn it into whatever future it likes. When the connection ends, every
//! request still waiting gets [`Gone`].

use std::collections::HashMap;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::geom::Provider;
use crate::wire::{
    Event, Hello, HelperFrame, Op, PROTOCOL, Policy, Reply, Request, read_frame, write_frame,
};

/// A reply and the images that followed it (`image`, then `trajectory`, as listed).
#[derive(Debug, Clone)]
pub struct Answer {
    pub reply: Reply,
    pub image: Option<Vec<u8>>,
    pub trajectory: Option<Vec<u8>>,
}

/// The connection ended before the reply came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gone;

type Waiter = Box<dyn FnOnce(Result<Answer, Gone>) + Send>;

pub struct Client {
    writer: Mutex<UnixStream>,
    pending: Arc<Mutex<HashMap<u64, Waiter>>>,
    next: AtomicU64,
    alive: Arc<AtomicBool>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Client {
    /// Connects and authenticates. `on_event` runs on the reader thread for each [`Event`].
    pub fn connect(
        socket: &Path,
        token: &str,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> std::io::Result<Self> {
        let mut stream = UnixStream::connect(socket)?;
        let hello = serde_json::to_vec(&Hello {
            token: token.to_owned(),
            protocol: PROTOCOL,
        })?;
        write_frame(&mut stream, &hello)?;
        let mut reader = stream.try_clone()?;
        let pending: Arc<Mutex<HashMap<u64, Waiter>>> = Arc::default();
        let alive = Arc::new(AtomicBool::new(true));
        let (p, a) = (pending.clone(), alive.clone());
        std::thread::Builder::new()
            .name("computer-helper-reader".into())
            .spawn(move || {
                while let Ok(frame) = read_frame(&mut reader) {
                    let Ok(frame) = serde_json::from_slice::<HelperFrame>(&frame) else {
                        break;
                    };
                    match frame {
                        HelperFrame::Event(e) => on_event(e),
                        HelperFrame::Reply(reply) => {
                            let mut image = None;
                            let mut trajectory = None;
                            if reply.image.is_some() {
                                let Ok(b) = read_frame(&mut reader) else {
                                    break;
                                };
                                image = Some(b);
                            }
                            if reply.trajectory.is_some() {
                                let Ok(b) = read_frame(&mut reader) else {
                                    break;
                                };
                                trajectory = Some(b);
                            }
                            let waiter = lock(&p).remove(&reply.id);
                            if let Some(w) = waiter {
                                w(Ok(Answer {
                                    reply,
                                    image,
                                    trajectory,
                                }));
                            }
                        }
                    }
                }
                a.store(false, Ordering::SeqCst);
                let waiting: Vec<Waiter> = lock(&p).drain().map(|(_, w)| w).collect();
                for w in waiting {
                    w(Err(Gone));
                }
            })?;
        Ok(Self {
            writer: Mutex::new(stream),
            pending,
            next: AtomicU64::new(1),
            alive,
        })
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// A fresh request id, for a request and the cancel that may follow it.
    pub fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::SeqCst)
    }

    /// Sends a request; `done` gets its answer, or [`Gone`].
    pub fn send(
        &self,
        id: u64,
        worker: &str,
        provider: Provider,
        policy: Policy,
        op: Op,
        done: impl FnOnce(Result<Answer, Gone>) + Send + 'static,
    ) {
        let req = Request {
            id,
            worker: worker.to_owned(),
            provider,
            policy,
            op,
        };
        lock(&self.pending).insert(id, Box::new(done));
        let sent = serde_json::to_vec(&req)
            .map_err(std::io::Error::from)
            .and_then(|b| write_frame(&mut *lock(&self.writer), &b));
        if sent.is_err() || !self.is_alive() {
            if let Some(w) = lock(&self.pending).remove(&id) {
                w(Err(Gone));
            }
        }
    }

    /// Sends a request whose answer nobody waits for (a cancel, a session's end).
    pub fn tell(&self, worker: &str, op: Op) {
        let id = self.next_id();
        self.send(id, worker, Provider::Claude, Policy::default(), op, |_| {});
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = lock(&self.writer).shutdown(std::net::Shutdown::Both);
    }
}
