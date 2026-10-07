//! Redacting secret values before any text leaves a session.
//!
//! A [`Redactor`] is built from the values of a project's secret files (and anything else that
//! must never be shown or stored, such as grants) and replaces every occurrence with
//! [`REDACTED`]. Both adapters apply the session's redactor to every event they emit, to the
//! stderr lines they keep and log, and to recorded stdio lines; the core applies the same
//! redactor to its own sinks (reports, artifacts, tool arguments) through [`Redactor::redact`].
//!
//! Streamed text (message, reasoning and command-output deltas) can split a secret across
//! chunks. Each stream therefore holds back the tail that could still be the start of a secret
//! until the next chunk proves otherwise, and flushes it when the item ends.

use std::borrow::Cow;
use std::collections::HashMap;

use aho_corasick::{AhoCorasick, MatchKind};

use crate::model::{ApprovalDecision, ProviderEvent};

/// What a secret is replaced with.
pub const REDACTED: &str = "[redacted]";

/// Values shorter than this are never treated as secrets: redacting them would mangle ordinary
/// text.
pub const MIN_SECRET_LEN: usize = 6;

/// Replaces secret values in text. Cheap to share (`Arc<Redactor>`); matching is a single pass
/// over the text, and text without a secret is returned as is, without allocating.
pub struct Redactor {
    secrets: Vec<String>,
    matcher: Option<AhoCorasick>,
    /// Length in bytes of the longest secret.
    longest: usize,
    /// Which bytes start a secret, to find where a split secret may begin.
    starts: [bool; 256],
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secrets themselves.
        f.debug_struct("Redactor")
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

impl Redactor {
    /// A redactor for `values`. Values shorter than [`MIN_SECRET_LEN`] (after trimming) are
    /// skipped; duplicates are merged.
    pub fn new<I, S>(values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut secrets: Vec<String> = values
            .into_iter()
            .map(Into::into)
            .filter(|value| value.trim().chars().count() >= MIN_SECRET_LEN)
            .collect();
        secrets.sort();
        secrets.dedup();
        let matcher = if secrets.is_empty() {
            None
        } else {
            match AhoCorasick::builder()
                .match_kind(MatchKind::LeftmostLongest)
                .build(&secrets)
            {
                Ok(matcher) => Some(matcher),
                // Only a pattern set far beyond any real secret list fails to build; the plain
                // search below then does the same work, slower.
                Err(err) => {
                    tracing::warn!(error = %err, "secret matcher unavailable; using plain search");
                    None
                }
            }
        };
        let mut starts = [false; 256];
        for secret in &secrets {
            starts[usize::from(secret.as_bytes()[0])] = true;
        }
        Self {
            longest: secrets.iter().map(String::len).max().unwrap_or(0),
            secrets,
            matcher,
            starts,
        }
    }

    /// This redactor plus `values`.
    pub fn with<I, S>(&self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(
            self.secrets
                .iter()
                .cloned()
                .chain(values.into_iter().map(Into::into)),
        )
    }

    /// Whether there is nothing to redact.
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }

    /// How many distinct secrets it redacts.
    pub fn len(&self) -> usize {
        self.secrets.len()
    }

    /// `text` with every secret replaced by [`REDACTED`]. Borrowed when there was none.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let mut out: Option<String> = None;
        let mut done = 0;
        for (start, end) in self.matches(text) {
            let out = out.get_or_insert_with(|| String::with_capacity(text.len()));
            out.push_str(&text[done..start]);
            out.push_str(REDACTED);
            done = end;
        }
        match out {
            None => Cow::Borrowed(text),
            Some(mut out) => {
                out.push_str(&text[done..]);
                Cow::Owned(out)
            }
        }
    }

    /// Redacts `text` in place; allocates only when it holds a secret.
    pub fn redact_in_place(&self, text: &mut String) {
        if let Cow::Owned(redacted) = self.redact(text) {
            *text = redacted;
        }
    }

    /// The next chunk of a stream, redacted. The tail that could be the start of a secret is
    /// kept in `pending` (the stream's state) until a later chunk or [`Redactor::flush`]
    /// settles it, so a secret split across chunks is still caught. Borrowed when the chunk
    /// passes through unchanged.
    pub fn push<'a>(&self, pending: &mut String, chunk: &'a str) -> Cow<'a, str> {
        if self.secrets.is_empty() {
            return Cow::Borrowed(chunk);
        }
        let joined: Cow<'a, str> = if pending.is_empty() {
            Cow::Borrowed(chunk)
        } else {
            let mut joined = std::mem::take(pending);
            joined.push_str(chunk);
            Cow::Owned(joined)
        };
        let text = joined.as_ref();
        let mut hold = self.hold_from(text, 0);
        let mut out: Option<String> = None;
        let mut done = 0;
        for (start, end) in self.matches(text) {
            if start >= hold {
                break;
            }
            let out = out.get_or_insert_with(|| String::with_capacity(text.len()));
            out.push_str(&text[done..start]);
            out.push_str(REDACTED);
            done = end;
            if end > hold {
                // A whole secret reached into the held tail: what is left after it may hold
                // the start of another.
                hold = self.hold_from(text, end);
            }
        }
        pending.push_str(&text[hold..]);
        match (out, joined) {
            (None, Cow::Borrowed(chunk)) if hold == chunk.len() => Cow::Borrowed(chunk),
            (None, Cow::Borrowed(chunk)) => Cow::Owned(chunk[..hold].to_owned()),
            (None, Cow::Owned(mut joined)) => {
                joined.truncate(hold);
                Cow::Owned(joined)
            }
            (Some(mut out), joined) => {
                out.push_str(&joined[done..hold]);
                Cow::Owned(out)
            }
        }
    }

    /// Ends a stream: the tail it held back, redacted. `None` when nothing was held.
    pub fn flush(&self, pending: &mut String) -> Option<String> {
        if pending.is_empty() {
            return None;
        }
        let tail = std::mem::take(pending);
        Some(self.redact(&tail).into_owned())
    }

    /// Byte ranges of the secrets in `text`, leftmost-longest, not overlapping.
    fn matches<'t>(&'t self, text: &'t str) -> Box<dyn Iterator<Item = (usize, usize)> + 't> {
        if self.secrets.is_empty() {
            return Box::new(std::iter::empty());
        }
        match &self.matcher {
            Some(matcher) => Box::new(
                matcher
                    .find_iter(text)
                    .map(|found| (found.start(), found.end())),
            ),
            None => Box::new(PlainMatches {
                secrets: &self.secrets,
                text,
                at: 0,
            }),
        }
    }

    /// Where the longest suffix of `text[from..]` that is the start (not the whole) of a
    /// secret begins; `text.len()` when there is none.
    fn hold_from(&self, text: &str, from: usize) -> usize {
        let bytes = text.as_bytes();
        let window = self.longest.saturating_sub(1);
        let first = from.max(bytes.len().saturating_sub(window));
        for at in first..bytes.len() {
            if !self.starts[usize::from(bytes[at])] {
                continue;
            }
            let tail = &bytes[at..];
            if self
                .secrets
                .iter()
                .any(|secret| secret.len() > tail.len() && secret.as_bytes().starts_with(tail))
            {
                // Secrets start on a character boundary, so `at` is one too.
                return at;
            }
        }
        bytes.len()
    }
}

/// Leftmost-longest search without the automaton (see [`Redactor::new`]).
struct PlainMatches<'a> {
    secrets: &'a [String],
    text: &'a str,
    at: usize,
}

impl Iterator for PlainMatches<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.text.get(self.at..)?;
        let (offset, len) = self
            .secrets
            .iter()
            .filter_map(|secret| rest.find(secret.as_str()).map(|at| (at, secret.len())))
            .min_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))?;
        let start = self.at + offset;
        self.at = start + len;
        Some((start, start + len))
    }
}

/// Configuration words that fill `.env` files but are not secrets; redacting them would
/// garble ordinary output.
const NOT_SECRETS: &[&str] = &[
    "true",
    "false",
    "yes",
    "no",
    "on",
    "off",
    "null",
    "none",
    "nil",
    "undefined",
    "enabled",
    "disabled",
    "localhost",
    "development",
    "production",
    "staging",
    "testing",
    "local",
    "debug",
    "verbose",
    "warning",
    "error",
    "trace",
    "default",
];

/// The values in a `.env`-style file that may be secrets: `KEY=value`, `export KEY=value`,
/// single, double or backtick quotes (double quotes unescape `\n`, `\t`, `\"`, `\\`; quoted
/// values may span lines), inline ` #` comments after unquoted values, and full-line comments.
///
/// Skipped: values shorter than [`MIN_SECRET_LEN`], numbers, booleans and common
/// configuration words ([`NOT_SECRETS`]). Each line of a multi-line value (a PEM key, without
/// its armor lines) is returned too, so a key printed line by line is still caught.
pub fn env_file_values(text: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line
            .strip_prefix("export")
            .filter(|rest| rest.starts_with([' ', '\t']))
            .map_or(line, str::trim_start);
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim_end();
        let is_key = key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
        if !is_key {
            continue;
        }
        let rest = rest.trim_start();
        let value = match rest.chars().next() {
            Some(quote @ ('"' | '\'' | '`')) => {
                let mut body = rest[1..].to_owned();
                // A quoted value runs to its closing quote, possibly lines later.
                loop {
                    if let Some(end) = closing_quote(&body, quote) {
                        body.truncate(end);
                        break;
                    }
                    match lines.next() {
                        Some(next) => {
                            body.push('\n');
                            body.push_str(next);
                        }
                        // Unterminated: take what there is.
                        None => break,
                    }
                }
                if quote == '"' { unescape(&body) } else { body }
            }
            _ => {
                let end = rest
                    .char_indices()
                    .find(|&(at, c)| {
                        c == '#' && rest[..at].ends_with(|b: char| b.is_ascii_whitespace())
                    })
                    .map_or(rest.len(), |(at, _)| at);
                rest[..end].trim().to_owned()
            }
        };
        if value.contains('\n') {
            values.extend(
                value
                    .lines()
                    .map(str::trim)
                    // PEM armor lines (`-----BEGIN … KEY-----`) are not secret.
                    .filter(|line| !line.starts_with("-----") && may_be_secret(line))
                    .map(str::to_owned),
            );
        }
        if may_be_secret(&value) {
            values.push(value);
        }
    }
    values.sort();
    values.dedup();
    values
}

/// Byte index of the quote that closes a value (not escaped, for double quotes).
fn closing_quote(body: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (at, c) in body.char_indices() {
        if quote == '"' && c == '\\' && !escaped {
            escaped = true;
            continue;
        }
        if c == quote && !escaped {
            return Some(at);
        }
        escaped = false;
    }
    None
}

fn unescape(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn may_be_secret(value: &str) -> bool {
    let value = value.trim();
    if value.chars().count() < MIN_SECRET_LEN {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if NOT_SECRETS.contains(&lower.as_str()) {
        return false;
    }
    // Numbers, however written (ports, sizes, versions such as 1.2.3).
    let is_number = value.chars().any(|c| c.is_ascii_digit())
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '_' | '-' | '+' | 'e' | 'E'));
    !is_number
}

/// Which kind of streamed text a held tail belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Stream {
    Message,
    Reasoning,
    CommandOutput,
}

/// Applies a session's redactor to its events, holding back split secrets per stream.
pub(crate) struct EventRedactor {
    redactor: std::sync::Arc<Redactor>,
    streams: HashMap<(Stream, String), String>,
}

impl EventRedactor {
    /// `None` when there is nothing to redact, so events pass through untouched.
    pub(crate) fn new(redactor: Option<std::sync::Arc<Redactor>>) -> Option<Self> {
        let redactor = redactor.filter(|redactor| !redactor.is_empty())?;
        Some(Self {
            redactor,
            streams: HashMap::new(),
        })
    }

    /// Redacts `event` and appends what to emit to `out`: nothing for a delta that is held
    /// back whole, the held tails of the streams an item or turn ends before its final event.
    pub(crate) fn apply(&mut self, mut event: ProviderEvent, out: &mut Vec<ProviderEvent>) {
        let redactor = self.redactor.clone();
        let delta = match &mut event {
            ProviderEvent::MessageDelta { item_id, text } => Some((Stream::Message, item_id, text)),
            ProviderEvent::ReasoningDelta { item_id, text } => {
                Some((Stream::Reasoning, item_id, text))
            }
            ProviderEvent::CommandOutputDelta { item_id, text } => {
                Some((Stream::CommandOutput, item_id, text))
            }
            _ => None,
        };
        if let Some((stream, item_id, text)) = delta {
            let pending = self.streams.entry((stream, item_id.clone())).or_default();
            if let Cow::Owned(redacted) = redactor.push(pending, text) {
                *text = redacted;
            }
            if !text.is_empty() {
                out.push(event);
            }
            return;
        }
        match &event {
            ProviderEvent::Message { item_id, .. } => self.flush(Stream::Message, item_id, out),
            ProviderEvent::Reasoning { item_id, .. } => self.flush(Stream::Reasoning, item_id, out),
            ProviderEvent::Command {
                item_id, status, ..
            } if *status != crate::model::ItemStatus::InProgress => {
                self.flush(Stream::CommandOutput, item_id, out)
            }
            ProviderEvent::TurnCompleted { .. }
            | ProviderEvent::Error { .. }
            | ProviderEvent::Exited { .. } => self.flush_all(out),
            _ => {}
        }
        redact_event(&redactor, &mut event);
        out.push(event);
    }

    /// Emits the held tail of one stream as a last delta.
    fn flush(&mut self, stream: Stream, item_id: &str, out: &mut Vec<ProviderEvent>) {
        if let Some(mut pending) = self.streams.remove(&(stream, item_id.to_owned()))
            && let Some(text) = self.redactor.flush(&mut pending)
        {
            out.push(delta(stream, item_id.to_owned(), text));
        }
    }

    fn flush_all(&mut self, out: &mut Vec<ProviderEvent>) {
        let mut streams: Vec<_> = self.streams.drain().collect();
        streams.sort_by(|a, b| a.0.1.cmp(&b.0.1));
        for ((stream, item_id), mut pending) in streams {
            if let Some(text) = self.redactor.flush(&mut pending) {
                out.push(delta(stream, item_id, text));
            }
        }
    }
}

fn delta(stream: Stream, item_id: String, text: String) -> ProviderEvent {
    match stream {
        Stream::Message => ProviderEvent::MessageDelta { item_id, text },
        Stream::Reasoning => ProviderEvent::ReasoningDelta { item_id, text },
        Stream::CommandOutput => ProviderEvent::CommandOutputDelta { item_id, text },
    }
}

/// Redacts every text field of a complete event.
pub fn redact_event(redactor: &Redactor, event: &mut ProviderEvent) {
    let r = |text: &mut String| redactor.redact_in_place(text);
    let opt = |text: &mut Option<String>| {
        if let Some(text) = text {
            redactor.redact_in_place(text);
        }
    };
    match event {
        ProviderEvent::SessionStarted { model, cwd, .. } => {
            opt(model);
            opt(cwd);
        }
        ProviderEvent::TurnStarted { .. }
        | ProviderEvent::Usage { .. }
        | ProviderEvent::ContextSize { .. }
        | ProviderEvent::CompactionStarted { .. }
        | ProviderEvent::RateLimits { .. }
        | ProviderEvent::Progress { .. }
        | ProviderEvent::TurnCompleted { .. } => {}
        ProviderEvent::MessageDelta { text, .. }
        | ProviderEvent::Message { text, .. }
        | ProviderEvent::ReasoningDelta { text, .. }
        | ProviderEvent::Reasoning { text, .. }
        | ProviderEvent::CommandOutputDelta { text, .. } => r(text),
        ProviderEvent::ToolCall {
            name,
            input,
            output,
            ..
        } => {
            r(name);
            opt(input);
            opt(output);
        }
        ProviderEvent::Command {
            command,
            cwd,
            output,
            ..
        } => {
            r(command);
            opt(cwd);
            opt(output);
        }
        ProviderEvent::FileChanges { changes, .. } => {
            for change in changes {
                r(&mut change.path);
            }
        }
        ProviderEvent::Image { path, prompt, .. } => {
            opt(path);
            opt(prompt);
        }
        ProviderEvent::Looked {
            cwd,
            reads,
            searches,
            ..
        } => {
            opt(cwd);
            for read in reads {
                r(&mut read.path);
            }
            for search in searches {
                opt(&mut search.pattern);
                opt(&mut search.scope);
                opt(&mut search.glob);
                search.hits.iter_mut().for_each(r);
            }
        }
        ProviderEvent::ApprovalRequested { request } => {
            r(&mut request.tool);
            opt(&mut request.command);
            opt(&mut request.cwd);
            for path in &mut request.paths {
                r(path);
            }
            opt(&mut request.reason);
            opt(&mut request.input);
        }
        ProviderEvent::ApprovalResolved { decision, .. } => {
            if let ApprovalDecision::Deny { message } = decision {
                r(message);
            }
        }
        ProviderEvent::Error { error } => {
            r(&mut error.message);
            opt(&mut error.code);
        }
        ProviderEvent::Notice { message, .. } => r(message),
        ProviderEvent::CompactionEnded { error, .. } => opt(error),
        ProviderEvent::Exited { stderr_tail, .. } => opt(stderr_tail),
    }
}
