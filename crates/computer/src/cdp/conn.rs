//! One browser's debugging connection: JSON commands and replies over a local WebSocket, with
//! the events that arrive between them kept for whoever asks. Page and frame sessions share the
//! one socket (`flatten` sessions), each command naming its session.

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tungstenite::{Message, WebSocket};

use crate::error::{CuError, CuResult, ErrorCode, err};

/// How long one command may take before the browser counts as not answering.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one read waits before the loop checks its deadline again.
const POLL: Duration = Duration::from_millis(10);
/// Events kept unread; the oldest go first, so a busy page can't grow it without bound.
const EVENTS_KEPT: usize = 4_096;

/// An event the browser sent.
#[derive(Debug, Clone)]
pub struct Event {
    pub session: Option<String>,
    pub method: String,
    pub params: Value,
}

pub struct Conn {
    ws: WebSocket<TcpStream>,
    next: u64,
    events: VecDeque<Event>,
}

impl Conn {
    /// Connects to `ws://127.0.0.1:<port><path>`, the browser endpoint the browser wrote in its
    /// profile's `DevToolsActivePort`.
    pub fn connect(port: u16, path: &str) -> CuResult<Self> {
        let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| {
            CuError::new(
                ErrorCode::AppNotResponding,
                format!("the browser's debugging port: {e}"),
            )
        })?;
        stream.set_nodelay(true).ok();
        let url = format!("ws://127.0.0.1:{port}{path}");
        let (ws, _) = tungstenite::client::client(url.as_str(), stream).map_err(|e| {
            CuError::new(
                ErrorCode::AppNotResponding,
                format!("the browser's debugging handshake: {e}"),
            )
        })?;
        ws.get_ref().set_read_timeout(Some(POLL)).ok();
        Ok(Self {
            ws,
            next: 1,
            events: VecDeque::new(),
        })
    }

    /// Sends a command and waits for its reply, keeping the events that come first.
    pub fn call(&mut self, session: Option<&str>, method: &str, params: Value) -> CuResult<Value> {
        self.call_within(session, method, params, CALL_TIMEOUT)
    }

    pub fn call_within(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> CuResult<Value> {
        let id = self.next;
        self.next += 1;
        let mut msg = json!({"id": id, "method": method, "params": params});
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        self.ws
            .send(Message::text(msg.to_string()))
            .map_err(|e| gone(&e))?;
        let deadline = Instant::now() + timeout;
        loop {
            match self.read_one()? {
                Some(v) if v.get("id").and_then(Value::as_u64) == Some(id) => {
                    if let Some(e) = v.get("error") {
                        let text = e.get("message").and_then(Value::as_str).unwrap_or("");
                        return err(ErrorCode::Failed, format!("{method}: {text}"));
                    }
                    return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                }
                Some(_) | None => {}
            }
            if Instant::now() >= deadline {
                return err(
                    ErrorCode::AppNotResponding,
                    format!("the browser didn't answer {method}"),
                );
            }
        }
    }

    /// Reads events for at most `d`.
    pub fn pump(&mut self, d: Duration) -> CuResult<()> {
        let deadline = Instant::now() + d;
        while Instant::now() < deadline {
            self.read_one()?;
        }
        Ok(())
    }

    /// Takes the events kept so far that `keep` wants; the others stay.
    pub fn take(&mut self, mut keep: impl FnMut(&Event) -> bool) -> Vec<Event> {
        let mut out = Vec::new();
        self.events.retain(|e| {
            if keep(e) {
                out.push(e.clone());
                false
            } else {
                true
            }
        });
        out
    }

    /// Reads one message if one comes within the poll interval: a reply is returned, an event
    /// is kept.
    fn read_one(&mut self) -> CuResult<Option<Value>> {
        let msg = match self.ws.read() {
            Ok(m) => m,
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                return Ok(None);
            }
            Err(e) => return Err(gone(&e)),
        };
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => {
                return err(
                    ErrorCode::AppNotResponding,
                    "the browser closed its connection",
                );
            }
            _ => return Ok(None),
        };
        let v: Value = serde_json::from_str(text.as_str())
            .map_err(|e| CuError::new(ErrorCode::Failed, format!("a browser message: {e}")))?;
        if v.get("id").is_some() {
            return Ok(Some(v));
        }
        if let Some(method) = v.get("method").and_then(Value::as_str) {
            self.events.push_back(Event {
                session: v
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                method: method.to_owned(),
                params: v.get("params").cloned().unwrap_or(Value::Null),
            });
            while self.events.len() > EVENTS_KEPT {
                self.events.pop_front();
            }
        }
        Ok(None)
    }
}

fn gone(e: &tungstenite::Error) -> CuError {
    CuError::new(
        ErrorCode::AppNotResponding,
        format!("the browser's debugging connection: {e}"),
    )
}
