//! Codex's own config writer: `config/value/write` on a short-lived `codex app-server` (stdio
//! JSON-RPC). It writes only the user's config; it refuses a project's file.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::ApplyError;

/// How long the app-server may take to answer one request.
const ANSWER: Duration = Duration::from_secs(30);
/// How long it may take to exit once its input is closed.
const EXIT: Duration = Duration::from_secs(5);

/// Writes `value` at `key_path` (`plugins."x@lib".enabled`) in the user's config through
/// `program app-server`; a JSON `null` removes the key (or table).
pub(super) fn write_value(
    program: &Path,
    env: &[(String, String)],
    key_path: &str,
    value: Value,
) -> Result<(), ApplyError> {
    let mut server = Server::start(program, env)?;
    let result = server.write(key_path, value);
    server.stop();
    result
}

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Receiver<String>,
    next_id: i64,
}

impl Server {
    fn start(program: &Path, env: &[(String, String)]) -> Result<Self, ApplyError> {
        let mut child = Command::new(program)
            .arg("app-server")
            .envs(env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| super::apply::spawn_error(&program.display().to_string(), &err))?;
        let stdin = child.stdin.take();
        let (tx, lines) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        let (tx, stderr) = mpsc::channel();
        if let Some(mut err) = child.stderr.take() {
            thread::spawn(move || {
                let mut text = String::new();
                let _ = err.read_to_string(&mut text);
                let _ = tx.send(text);
            });
        }
        Ok(Self {
            child,
            stdin,
            lines,
            stderr,
            next_id: 1,
        })
    }

    fn write(&mut self, key_path: &str, value: Value) -> Result<(), ApplyError> {
        self.call(
            "initialize",
            json!({ "clientInfo": {
                "name": "brigadier",
                "title": "Brigadier",
                "version": env!("CARGO_PKG_VERSION"),
            } }),
        )?;
        self.send(&json!({ "jsonrpc": "2.0", "method": "initialized" }))?;
        // No `filePath`: the user's own config.toml.
        self.call(
            "config/value/write",
            json!({ "keyPath": key_path, "value": value, "mergeStrategy": "upsert" }),
        )?;
        Ok(())
    }

    fn send(&mut self, message: &Value) -> Result<(), ApplyError> {
        let stdin = self.stdin.as_mut().ok_or_else(|| gone(None))?;
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .map_err(|_| gone(None))
    }

    /// Sends a request and waits for its answer, passing over anything else Codex says.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, ApplyError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let deadline = Instant::now() + ANSWER;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = match self.lines.recv_timeout(left) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(ApplyError::Failed(
                        "Codex didn't answer in time while saving the setting.".to_owned(),
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => return Err(gone(self.stderr_line())),
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message.get("id").and_then(Value::as_i64) != Some(id)
                || message.get("method").is_some()
            {
                continue;
            }
            if let Some(error) = message.get("error") {
                let why = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no reason given");
                return Err(ApplyError::Cli(format!(
                    "Codex couldn't save the setting: {why}"
                )));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// The app-server's last words, once it has exited.
    fn stderr_line(&mut self) -> Option<String> {
        let _ = wait(&mut self.child, EXIT);
        let text = self.stderr.recv_timeout(Duration::from_millis(200)).ok()?;
        super::apply::stderr_line(&text)
    }

    /// Closes its input, which ends it; kills it if it lingers.
    fn stop(mut self) {
        drop(self.stdin.take());
        if !wait(&mut self.child, EXIT) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Waits up to `limit` for `child` to exit; whether it did.
fn wait(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => return false,
        }
    }
}

fn gone(why: Option<String>) -> ApplyError {
    ApplyError::Cli(match why {
        Some(why) => format!("Codex stopped while saving the setting: {why}"),
        None => "Codex stopped while saving the setting.".to_owned(),
    })
}
