//! Raw stdio recordings, the source of replay fixtures.
//!
//! A recording is JSONL: a header, then one line per stdio line in either direction with its
//! offset from the start. Personal data is scrubbed as it is written, so a recording can be
//! committed as a fixture: emails, account and organization identifiers, the home directory and
//! user name (also in object keys, and when a stream splits a path) and the contents of the
//! user's Codex configuration never reach the file. With a [`Redactor`], secret values never do
//! either, including a secret streamed across several delta lines: each delta stream holds back
//! the tail that may start a secret, and a line carrying what was held is written when the
//! stream ends.

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::ProviderKind;
use crate::redact::Redactor;

/// Fixture format version.
pub const FORMAT: u32 = 1;

/// Keys whose string values identify the user or their account.
const PRIVATE_KEYS: &[&str] = &[
    "email",
    "orgId",
    "organization",
    "orgName",
    "accountId",
    "account_id",
    "accountUuid",
    "chatgptAccountId",
    "installationId",
    "userId",
    "user_id",
    "serverName",
    "userAgent",
    "user_agent",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// Brigadier → CLI (stdin).
    In,
    /// CLI → Brigadier (stdout).
    Out,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub fixture: u32,
    pub provider: ProviderKind,
    pub cli_version: Option<String>,
    pub recorded_at_ms: i64,
    /// What the recording shows, for the replay picker.
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    /// Milliseconds since the recording started.
    pub t: u64,
    pub dir: Direction,
    pub line: String,
}

/// Writes a recording on its own thread, so stdio handling never waits on the disk.
pub struct Recorder {
    started: Instant,
    lines: mpsc::Sender<Line>,
    writer: std::thread::JoinHandle<()>,
}

impl Recorder {
    /// Starts a recording at `path`. Lines are scrubbed, and redacted with `redactor`, before
    /// they are written.
    pub fn create(
        path: &Path,
        header: &Header,
        redactor: Option<Arc<Redactor>>,
    ) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = BufWriter::new(std::fs::File::create(path)?);
        serde_json::to_writer(&mut file, header)?;
        file.write_all(b"\n")?;
        file.flush()?;

        let (lines, rx) = mpsc::channel::<Line>();
        let redactor = redactor.filter(|redactor| !redactor.is_empty());
        let mut streams = redactor
            .clone()
            .map(|redactor| DeltaStreams::new(header.provider, redactor));
        let scrubber = Scrubber::new(redactor);
        let path = path.to_owned();
        let writer = std::thread::Builder::new()
            .name("cli-recorder".into())
            .spawn(move || {
                for line in rx {
                    for text in scrubber.scrub_line(line.dir, &line.line, streams.as_mut()) {
                        let row = Line {
                            t: line.t,
                            dir: line.dir,
                            line: text,
                        };
                        let written = serde_json::to_writer(&mut file, &row)
                            .map_err(std::io::Error::from)
                            .and_then(|()| file.write_all(b"\n"))
                            .and_then(|()| file.flush());
                        if let Err(err) = written {
                            tracing::warn!(
                                path = %path.display(),
                                error = %err,
                                "recording stopped"
                            );
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            started: Instant::now(),
            lines,
            writer,
        })
    }

    pub fn record(&self, dir: Direction, line: &str) {
        let _ = self.lines.send(Line {
            t: self.started.elapsed().as_millis() as u64,
            dir,
            line: line.to_owned(),
        });
    }

    /// Waits until every queued line has been scrubbed and written. Recording tools use this
    /// before reading back a scrubbed native transcript (D9(c)); live adapters need not wait.
    pub fn finish(self) -> std::thread::Result<()> {
        drop(self.lines);
        self.writer.join()
    }
}

/// A parsed recording.
pub struct Recording {
    pub header: Header,
    pub lines: Vec<Line>,
}

impl Recording {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut rows = text.lines().filter(|line| !line.trim().is_empty());
        let header: Header = rows
            .next()
            .ok_or("the recording is empty")
            .and_then(|line| serde_json::from_str(line).map_err(|_| "bad recording header"))?;
        if header.fixture != FORMAT {
            return Err(format!("unsupported recording format {}", header.fixture));
        }
        let lines = rows
            .enumerate()
            .map(|(index, row)| {
                serde_json::from_str(row).map_err(|err| format!("line {}: {err}", index + 2))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { header, lines })
    }
}

/// Removes personal data and secrets from recorded lines.
struct Scrubber {
    home: Option<String>,
    /// The home directory's last component, replaced wherever it appears as a word: a streamed
    /// path can arrive split across deltas, out of reach of the home replacement.
    user: Option<String>,
    redactor: Option<Arc<Redactor>>,
}

impl Scrubber {
    fn new(redactor: Option<Arc<Redactor>>) -> Self {
        let home = dirs::home_dir();
        Self {
            redactor,
            user: home
                .as_ref()
                .and_then(|home| home.file_name())
                .and_then(|name| name.to_str())
                .filter(|name| name.len() >= 3)
                .map(str::to_owned),
            home: home
                .map(|home| home.display().to_string())
                .filter(|home| home.len() > 1),
        }
    }

    /// The lines to record for `line`: the line itself, scrubbed, after any line that ends a
    /// delta stream's held tail.
    fn scrub_line(
        &self,
        dir: Direction,
        line: &str,
        streams: Option<&mut DeltaStreams>,
    ) -> Vec<String> {
        match serde_json::from_str::<Value>(line) {
            Ok(mut value) => {
                redact_codex_config(&mut value);
                let mut rows = match streams {
                    Some(streams) if dir == Direction::Out => streams.apply(&mut value),
                    _ => Vec::new(),
                };
                rows.push(value);
                rows.into_iter()
                    .map(|mut row| {
                        self.scrub_value(&mut row, None);
                        row.to_string()
                    })
                    .collect()
            }
            Err(_) => vec![self.scrub_text(line)],
        }
    }

    fn scrub_value(&self, value: &mut Value, key: Option<&str>) {
        match value {
            Value::String(text) => {
                if key.is_some_and(|key| PRIVATE_KEYS.contains(&key)) {
                    *text = "redacted".into();
                } else {
                    *text = self.scrub_text(text);
                }
            }
            Value::Array(items) => items
                .iter_mut()
                .for_each(|item| self.scrub_value(item, key)),
            Value::Object(map) => {
                let entries = std::mem::take(map);
                for (key, mut value) in entries {
                    self.scrub_value(&mut value, Some(&key));
                    map.insert(self.scrub_text(&key), value);
                }
            }
            _ => {}
        }
    }

    fn scrub_text(&self, text: &str) -> String {
        let text = match &self.redactor {
            Some(redactor) => redactor.redact(text),
            None => std::borrow::Cow::Borrowed(text),
        };
        let mut text = match &self.home {
            Some(home) => text.replace(home.as_str(), "~"),
            None => text.into_owned(),
        };
        if text.contains('@') {
            text = scrub_emails(&text);
        }
        if let Some(user) = &self.user
            && text.contains(user.as_str())
        {
            text = replace_word(&text, user, "user");
        }
        text
    }
}

/// The delta streams of a recording, each holding back the tail that may start a secret.
struct DeltaStreams {
    provider: ProviderKind,
    redactor: Arc<Redactor>,
    streams: HashMap<String, HeldDelta>,
}

struct HeldDelta {
    /// The item (Codex) or content block (Claude) the stream belongs to.
    item: String,
    /// Where the delta text sits in the line.
    pointer: &'static str,
    pending: String,
    /// The stream's last line, the shape of the line that carries the held tail at the end.
    last: Value,
}

impl DeltaStreams {
    fn new(provider: ProviderKind, redactor: Arc<Redactor>) -> Self {
        Self {
            provider,
            redactor,
            streams: HashMap::new(),
        }
    }

    /// Redacts a delta line through its stream, or ends the streams `line` closes, returning
    /// lines with their held tails to record before it.
    fn apply(&mut self, line: &mut Value) -> Vec<Value> {
        if let Some((key, item, pointer)) = delta_location(self.provider, line) {
            let held = self.streams.entry(key).or_insert_with(|| HeldDelta {
                item,
                pointer,
                pending: String::new(),
                last: Value::Null,
            });
            if let Some(Value::String(text)) = line.pointer_mut(pointer) {
                let redacted = self.redactor.push(&mut held.pending, text).into_owned();
                *text = redacted;
            }
            held.last = line.clone();
            return Vec::new();
        }
        let Some(ended) = stream_end(self.provider, line) else {
            return Vec::new();
        };
        let keys: Vec<String> = self
            .streams
            .iter()
            .filter(|(_, held)| ended.as_deref().is_none_or(|item| held.item == item))
            .map(|(key, _)| key.clone())
            .collect();
        let mut rows = Vec::new();
        for key in keys {
            let Some(mut held) = self.streams.remove(&key) else {
                continue;
            };
            if let Some(tail) = self.redactor.flush(&mut held.pending)
                && let Some(text) = held.last.pointer_mut(held.pointer)
            {
                *text = Value::String(tail);
                rows.push(held.last);
            }
        }
        rows
    }
}

/// For a streamed delta line: its stream key, its item and where its text is.
fn delta_location(provider: ProviderKind, line: &Value) -> Option<(String, String, &'static str)> {
    match provider {
        ProviderKind::Codex => {
            let method = line.get("method")?.as_str()?;
            let params = line.get("params")?;
            params.get("delta")?.as_str()?;
            let item = params.get("itemId")?.as_str()?;
            if !method.starts_with("item/") {
                return None;
            }
            let part = ["summaryIndex", "contentIndex"]
                .iter()
                .filter_map(|key| params.get(key))
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join(":");
            Some((
                format!("{method}|{item}|{part}"),
                item.to_owned(),
                "/params/delta",
            ))
        }
        ProviderKind::Claude => {
            if line.get("type")?.as_str()? != "stream_event" {
                return None;
            }
            let event = line.get("event")?;
            if event.get("type")?.as_str()? != "content_block_delta" {
                return None;
            }
            let pointer = match event.pointer("/delta/type")?.as_str()? {
                "text_delta" => "/event/delta/text",
                "thinking_delta" => "/event/delta/thinking",
                "input_json_delta" => "/event/delta/partial_json",
                _ => return None,
            };
            let item = claude_block(line, event);
            Some((format!("{item}|{pointer}"), item, pointer))
        }
    }
}

/// The streams a line ends: `Some(item)` for one item's, `None` for all of them.
fn stream_end(provider: ProviderKind, line: &Value) -> Option<Option<String>> {
    match provider {
        ProviderKind::Codex => match line.get("method")?.as_str()? {
            "item/completed" => Some(Some(line.pointer("/params/item/id")?.as_str()?.to_owned())),
            "turn/completed" => Some(None),
            _ => None,
        },
        ProviderKind::Claude => match line.get("type")?.as_str()? {
            "stream_event" => {
                let event = line.get("event")?;
                match event.get("type")?.as_str()? {
                    "content_block_stop" => Some(Some(claude_block(line, event))),
                    "message_stop" => Some(None),
                    _ => None,
                }
            }
            "result" => Some(None),
            _ => None,
        },
    }
}

/// A Claude content block: its index within the (sub-agent's) message.
fn claude_block(line: &Value, event: &Value) -> String {
    format!(
        "{}:{}",
        line.get("parent_tool_use_id")
            .and_then(Value::as_str)
            .unwrap_or("main"),
        event.get("index").and_then(Value::as_u64).unwrap_or(0)
    )
}

/// Empties a Codex `config/read` result: it is the user's whole configuration (projects, MCP
/// servers, preferences), and replaying a session never needs it.
fn redact_codex_config(value: &mut Value) {
    let Some(result) = value.get_mut("result").and_then(Value::as_object_mut) else {
        return;
    };
    if result.contains_key("config") && result.contains_key("origins") {
        result.clear();
        result.insert("config".into(), Value::Object(Default::default()));
        result.insert("origins".into(), Value::Object(Default::default()));
    }
}

/// Replaces `word` where it is not part of a longer alphanumeric word.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(word) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + word.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_some_and(is_word) || after.is_some_and(is_word) {
            out.push_str(word);
        } else {
            out.push_str(with);
        }
        rest = &rest[at + word.len()..];
    }
    out.push_str(rest);
    out
}

/// Replaces anything shaped like an email address.
fn scrub_emails(text: &str) -> String {
    let is_local = |c: char| c.is_ascii_alphanumeric() || "._%+-".contains(c);
    let is_domain = |c: char| c.is_ascii_alphanumeric() || ".-".contains(c);
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '@' {
            let local_len = out.chars().rev().take_while(|c| is_local(*c)).count();
            let domain_len = chars[index + 1..]
                .iter()
                .take_while(|c| is_domain(**c))
                .count();
            let domain: String = chars[index + 1..index + 1 + domain_len].iter().collect();
            let domain = domain.trim_end_matches('.');
            if local_len > 0 && domain.contains('.') {
                let keep = out.chars().count() - local_len;
                out = out.chars().take(keep).collect();
                out.push_str("user@example.com");
                index += 1 + domain.len();
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

/// Where recordings made from the Inspector are kept.
pub fn recordings_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("recordings")
}
