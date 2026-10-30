//! `~/.claude.json`: JSON as Claude writes it (`JSON.stringify(config, null, 2)`), edited by
//! byte ranges found with a small scanner; `serde_json` only checks that the file parses.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::cli::CliEnv;
use crate::trust::{State, Trust, TrustBefore};
use crate::{Error, Result};

/// Claude Code's folder trust: `projects["<path>"].hasTrustDialogAccepted`. It writes the file
/// under a `proper-lockfile` lock, the folder `<file>.lock`; Brigadier takes the same lock.
pub(super) static TRUST: Trust = Trust {
    file,
    lock: ".lock",
    state,
    trust,
    undo,
    // Claude has its settings file.
    set_up: Path::exists,
    account_file: Some(|home| home.join(".claude.json")),
};

/// `$CLAUDE_CONFIG_DIR/.claude.json`, else `~/.claude.json`.
fn file(env: &CliEnv) -> Option<PathBuf> {
    match env.var("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join(".claude.json")),
        None => env.home().map(|home| home.join(".claude.json")),
    }
}

const KEY: &str = "hasTrustDialogAccepted";

/// A member of an object: its key and the ranges of the key (with its quotes) and value.
struct Member {
    key: String,
    key_span: Range<usize>,
    value: Range<usize>,
}

/// An object's members and the positions of its braces.
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

impl Object {
    fn get(&self, key: &str) -> Option<&Member> {
        self.members.iter().find(|member| member.key == key)
    }
}

fn invalid(why: &str) -> Error {
    Error::Invalid(format!(
        "Claude's settings file can't be edited safely: {why}"
    ))
}

fn parse(text: &str) -> Result<Option<Object>> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(text).map_err(|err| invalid(&err.to_string()))?;
    if !value.is_object() {
        return Err(invalid("it is not a JSON object"));
    }
    let open = skip_ws(text, 0);
    object(text, open).map(Some)
}

pub(crate) fn state(text: &str, folder: &str) -> Result<State> {
    let Some(root) = parse(text)? else {
        return Ok(State::NoProjects);
    };
    let Some(projects) = root.get("projects") else {
        return Ok(State::NoProjects);
    };
    if !text[projects.value.clone()].starts_with('{') {
        return Err(invalid("\"projects\" is not an object"));
    }
    let projects = object(text, projects.value.start)?;
    let Some(entry) = projects.get(folder) else {
        return Ok(State::NoEntry);
    };
    if !text[entry.value.clone()].starts_with('{') {
        return Err(invalid("the folder's entry is not an object"));
    }
    let entry = object(text, entry.value.start)?;
    Ok(match entry.get(KEY) {
        None => State::NoKey,
        Some(member) if &text[member.value.clone()] == "true" => State::Trusted,
        Some(member) => State::Other {
            raw: text[member.value.clone()].to_owned(),
            span: member.value.clone(),
        },
    })
}

pub(crate) fn trust(text: &str, folder: &str, state: &State) -> Result<String> {
    let key = serde_json::to_string(folder).map_err(|err| invalid(&err.to_string()))?;
    let entry = |depth: usize| {
        format!(
            "{{\n{}\"{KEY}\": true\n{}}}",
            indent(depth + 1),
            indent(depth)
        )
    };
    Ok(match state {
        State::Trusted => text.to_owned(),
        State::NoProjects => match parse(text)? {
            None => format!(
                "{{\n  \"projects\": {{\n    {key}: {}\n  }}\n}}\n",
                entry(2)
            ),
            Some(root) => {
                let value = format!("{{\n{}{key}: {}\n{}}}", indent(2), entry(2), indent(1));
                insert(text, &root, 0, "\"projects\"", &value)
            }
        },
        State::NoEntry => {
            let projects = projects(text)?;
            insert(text, &projects, 1, &key, &entry(2))
        }
        State::NoKey => {
            let entry = entry_object(text, folder)?;
            insert(text, &entry, 2, &format!("\"{KEY}\""), "true")
        }
        State::Other { span, .. } => splice(text, span.clone(), "true"),
    })
}

pub(crate) fn undo(text: &str, folder: &str, before: &TrustBefore) -> Result<Option<String>> {
    if !matches!(state(text, folder)?, State::Trusted) {
        return Ok(None);
    }
    let entry = entry_object(text, folder)?;
    let index = entry
        .members
        .iter()
        .position(|member| member.key == KEY)
        .ok_or_else(|| invalid("the trust key went missing"))?;
    Ok(Some(match before {
        TrustBefore::NoEntry if entry.members.len() == 1 => {
            let projects = projects(text)?;
            let at = projects
                .members
                .iter()
                .position(|member| member.key == folder)
                .ok_or_else(|| invalid("the folder's entry went missing"))?;
            remove(text, &projects, at)
        }
        TrustBefore::NoEntry | TrustBefore::NoKey => remove(text, &entry, index),
        TrustBefore::Value { raw } => splice(text, entry.members[index].value.clone(), raw),
    }))
}

fn projects(text: &str) -> Result<Object> {
    let root = parse(text)?.ok_or_else(|| invalid("it is empty"))?;
    let member = root
        .get("projects")
        .ok_or_else(|| invalid("\"projects\" went missing"))?;
    object(text, member.value.start)
}

fn entry_object(text: &str, folder: &str) -> Result<Object> {
    let projects = projects(text)?;
    let member = projects
        .get(folder)
        .ok_or_else(|| invalid("the folder's entry went missing"))?;
    object(text, member.value.start)
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn splice(text: &str, range: Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..range.start]);
    out.push_str(with);
    out.push_str(&text[range.end..]);
    out
}

/// Adds `"key": value` as the last member of `object`, which sits at `depth`.
fn insert(text: &str, object: &Object, depth: usize, key: &str, value: &str) -> String {
    match object.members.last() {
        Some(last) => splice(
            text,
            last.value.end..last.value.end,
            &format!(",\n{}{key}: {value}", indent(depth + 1)),
        ),
        None => splice(
            text,
            object.open..object.close + 1,
            &format!(
                "{{\n{}{key}: {value}\n{}}}",
                indent(depth + 1),
                indent(depth)
            ),
        ),
    }
}

/// Removes member `index` of `object` with its separator.
fn remove(text: &str, object: &Object, index: usize) -> String {
    let members = &object.members;
    let member = &members[index];
    let range = if index > 0 {
        members[index - 1].value.end..member.value.end
    } else if let Some(next) = members.get(1) {
        member.key_span.start..next.key_span.start
    } else {
        object.open..object.close + 1
    };
    let with = if members.len() == 1 { "{}" } else { "" };
    splice(text, range, with)
}

fn skip_ws(text: &str, mut at: usize) -> usize {
    let bytes = text.as_bytes();
    while at < bytes.len() && matches!(bytes[at], b' ' | b'\t' | b'\n' | b'\r') {
        at += 1;
    }
    at
}

/// The end of the string starting at `at` (its opening quote), past the closing quote.
fn string_end(text: &str, at: usize) -> Result<usize> {
    let bytes = text.as_bytes();
    let mut i = at + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err(invalid("a string never ends"))
}

/// The end of the value starting at `at`.
fn value_end(text: &str, at: usize) -> Result<usize> {
    let bytes = text.as_bytes();
    match bytes.get(at) {
        Some(b'"') => string_end(text, at),
        Some(b'{' | b'[') => {
            let mut depth = 0usize;
            let mut i = at;
            while i < bytes.len() {
                match bytes[i] {
                    b'"' => {
                        i = string_end(text, i)?;
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return Ok(i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            Err(invalid("an object never ends"))
        }
        Some(_) => {
            let mut i = at;
            while i < bytes.len()
                && !matches!(bytes[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
            {
                i += 1;
            }
            Ok(i)
        }
        None => Err(invalid("a value is missing")),
    }
}

/// The object whose `{` is at `open`.
fn object(text: &str, open: usize) -> Result<Object> {
    let bytes = text.as_bytes();
    if bytes.get(open) != Some(&b'{') {
        return Err(invalid("an object was expected"));
    }
    let mut members = Vec::new();
    let mut at = skip_ws(text, open + 1);
    if bytes.get(at) == Some(&b'}') {
        return Ok(Object {
            open,
            close: at,
            members,
        });
    }
    loop {
        let key_end = string_end(text, at)?;
        let key: String =
            serde_json::from_str(&text[at..key_end]).map_err(|err| invalid(&err.to_string()))?;
        let colon = skip_ws(text, key_end);
        if bytes.get(colon) != Some(&b':') {
            return Err(invalid("a key has no value"));
        }
        let start = skip_ws(text, colon + 1);
        let end = value_end(text, start)?;
        members.push(Member {
            key,
            key_span: at..key_end,
            value: start..end,
        });
        let next = skip_ws(text, end);
        match bytes.get(next) {
            Some(b',') => at = skip_ws(text, next + 1),
            Some(b'}') => {
                return Ok(Object {
                    open,
                    close: next,
                    members,
                });
            }
            _ => return Err(invalid("an object is malformed")),
        }
    }
}
