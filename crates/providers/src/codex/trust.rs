//! `config.toml`: parsed with `toml_edit` for its spans only; the edit is a splice.

use std::ops::Range;
use std::path::{Path, PathBuf};

use toml_edit::{Document, Item, Table};

use crate::cli::CliEnv;
use crate::trust::{Edit, State, Trust, TrustBefore, edit};
use crate::{Error, Result};

/// Codex's folder trust: `[projects."<path>"] trust_level = "trusted"`.
pub(super) static TRUST: Trust = Trust {
    file,
    lock: LOCK,
    state,
    trust,
    undo,
    // Codex has its folder.
    set_up: |file| file.parent().is_some_and(Path::is_dir),
    // An extra account's `config.toml` is the user's own, linked.
    account_file: None,
};

/// Codex takes no lock for its `config.toml`: Brigadier's own writes take one beside it
/// (`config.toml.brigadier.lock`), so two of them can't lose each other's entry.
const LOCK: &str = ".brigadier.lock";

/// `$CODEX_HOME/config.toml`, by default `~/.codex/config.toml`.
pub(super) fn file(env: &CliEnv) -> Option<PathBuf> {
    env.var("CODEX_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| env.home().map(|home| home.join(".codex")))
        .map(|dir| dir.join("config.toml"))
}

const KEY: &str = "trust_level";
const TRUSTED: &str = "trusted";

fn invalid(why: &str) -> Error {
    Error::Invalid(format!("Codex's config.toml can't be edited safely: {why}"))
}

fn parse(text: &str) -> Result<Document<&str>> {
    Document::parse(text).map_err(|err| invalid(err.message()))
}

/// The folder's table and the `projects` table, when they are standard tables.
fn tables<'a>(
    doc: &'a Document<&str>,
    folder: &str,
) -> Result<(Option<&'a Table>, Option<&'a Table>)> {
    let Some(projects) = doc.as_table().get("projects") else {
        return Ok((None, None));
    };
    let projects = match projects {
        Item::Table(table) if !table.is_dotted() => table,
        _ => return Err(invalid("[projects] is written inline or with dotted keys")),
    };
    match projects.get(folder) {
        None => Ok((Some(projects), None)),
        Some(Item::Table(table)) if !table.is_dotted() && !table.is_implicit() => {
            Ok((Some(projects), Some(table)))
        }
        Some(_) => Err(invalid(
            "the folder's entry is written inline or with dotted keys",
        )),
    }
}

pub(crate) fn state(text: &str, folder: &str) -> Result<State> {
    let doc = parse(text)?;
    let (projects, entry) = tables(&doc, folder)?;
    let Some(entry) = entry else {
        return Ok(if projects.is_some() {
            State::NoEntry
        } else {
            State::NoProjects
        });
    };
    Ok(match entry.get(KEY) {
        None => State::NoKey,
        Some(item) if item.as_str() == Some(TRUSTED) => State::Trusted,
        Some(item) => {
            let span = item
                .span()
                .ok_or_else(|| invalid("a value has no position"))?;
            State::Other {
                raw: text[span.clone()].to_owned(),
                span,
            }
        }
    })
}

/// The table header a folder's entry gets: `[projects."<folder>"]`.
fn header(folder: &str) -> String {
    format!("[projects.{}]", basic_string(folder))
}

/// A TOML basic string.
fn basic_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) fn trust(text: &str, folder: &str, state: &State) -> Result<String> {
    let line = format!("{KEY} = \"{TRUSTED}\"");
    let new = match state {
        State::Trusted => return Ok(text.to_owned()),
        State::NoProjects | State::NoEntry => {
            let mut new = text.to_owned();
            if !new.is_empty() && !new.ends_with('\n') {
                new.push('\n');
            }
            if !new.is_empty() {
                new.push('\n');
            }
            new.push_str(&format!("{}\n{line}\n", header(folder)));
            new
        }
        State::NoKey => {
            let doc = parse(text)?;
            let (_, entry) = tables(&doc, folder)?;
            let span = entry
                .and_then(Table::span)
                .ok_or_else(|| invalid("the folder's table has no position"))?;
            // Right after its header line.
            let at = text[span.end..]
                .find('\n')
                .map_or(text.len(), |i| span.end + i + 1);
            let mut new = text[..at].to_owned();
            if !new.ends_with('\n') {
                new.push('\n');
            }
            new.push_str(&line);
            new.push('\n');
            new.push_str(&text[at..]);
            new
        }
        State::Other { span, .. } => {
            format!("{}\"{TRUSTED}\"{}", &text[..span.start], &text[span.end..])
        }
    };
    // What was spliced in must read back as exactly that.
    match self::state(&new, folder)? {
        State::Trusted => Ok(new),
        _ => Err(invalid("the edit did not read back as trusted")),
    }
}

const HOOK_KEY: &str = "trusted_hash";

/// A hook's state table, `[hooks.state."<key>"]`, when it and the tables above it are
/// standard tables (an implicit `hooks` or `hooks.state` counts).
fn hook_table<'a>(doc: &'a Document<&str>, key: &str) -> Result<Option<&'a Table>> {
    let standard = |item: &'a Item, what: &str| match item {
        Item::Table(table) if !table.is_dotted() => Ok(table),
        _ => Err(invalid(&format!(
            "{what} is written inline or with dotted keys"
        ))),
    };
    let Some(hooks) = doc.as_table().get("hooks") else {
        return Ok(None);
    };
    let Some(states) = standard(hooks, "[hooks]")?.get("state") else {
        return Ok(None);
    };
    match standard(states, "[hooks.state]")?.get(key) {
        None => Ok(None),
        Some(Item::Table(table)) if !table.is_dotted() && !table.is_implicit() => Ok(Some(table)),
        Some(_) => Err(invalid(
            "the hook's state is written inline or with dotted keys",
        )),
    }
}

/// Where the trust of the hook `key` stands: [`State::Trusted`] only with `hash`.
pub(crate) fn hook_state(text: &str, key: &str, hash: &str) -> Result<State> {
    let doc = parse(text)?;
    let Some(table) = hook_table(&doc, key)? else {
        return Ok(State::NoEntry);
    };
    Ok(match table.get(HOOK_KEY) {
        None => State::NoKey,
        Some(item) if item.as_str() == Some(hash) => State::Trusted,
        Some(item) => {
            let span = item
                .span()
                .ok_or_else(|| invalid("a value has no position"))?;
            State::Other {
                raw: text[span.clone()].to_owned(),
                span,
            }
        }
    })
}

/// `text` with the hook `key` trusted at `hash`: a table of its own at the end, a line
/// right after its header, or its old hash replaced in place.
pub(crate) fn trust_hook(text: &str, key: &str, hash: &str, state: &State) -> Result<String> {
    let line = format!("{HOOK_KEY} = {}", basic_string(hash));
    let new = match state {
        State::Trusted => return Ok(text.to_owned()),
        State::NoProjects | State::NoEntry => {
            let mut new = text.to_owned();
            if !new.is_empty() && !new.ends_with('\n') {
                new.push('\n');
            }
            if !new.is_empty() {
                new.push('\n');
            }
            new.push_str(&format!("[hooks.state.{}]\n{line}\n", basic_string(key)));
            new
        }
        State::NoKey => {
            let doc = parse(text)?;
            let span = hook_table(&doc, key)?
                .and_then(Table::span)
                .ok_or_else(|| invalid("the hook's table has no position"))?;
            let at = text[span.end..]
                .find('\n')
                .map_or(text.len(), |i| span.end + i + 1);
            let mut new = text[..at].to_owned();
            if !new.ends_with('\n') {
                new.push('\n');
            }
            new.push_str(&line);
            new.push('\n');
            new.push_str(&text[at..]);
            new
        }
        State::Other { span, .. } => format!(
            "{}{}{}",
            &text[..span.start],
            basic_string(hash),
            &text[span.end..]
        ),
    };
    match hook_state(&new, key, hash)? {
        State::Trusted => Ok(new),
        _ => Err(invalid("the edit did not read back as trusted")),
    }
}

pub(crate) fn undo(text: &str, folder: &str, before: &TrustBefore) -> Result<Option<String>> {
    if !matches!(state(text, folder)?, State::Trusted) {
        return Ok(None);
    }
    let doc = parse(text)?;
    let (_, Some(entry)) = tables(&doc, folder)? else {
        return Ok(None);
    };
    let item = entry
        .get(KEY)
        .ok_or_else(|| invalid("the trust key went missing"))?;
    let value = item
        .span()
        .ok_or_else(|| invalid("a value has no position"))?;
    let line = line_of(text, value.clone());
    let ours = format!("{KEY} = \"{TRUSTED}\"");
    let new = match before {
        TrustBefore::Value { raw } => {
            format!("{}{raw}{}", &text[..value.start], &text[value.end..])
        }
        TrustBefore::NoEntry if entry.len() == 1 => {
            let header = entry
                .span()
                .ok_or_else(|| invalid("the folder's table has no position"))?;
            // The table runs to the next table's header, or the end.
            let end = next_table_start(doc.as_table(), header.end).unwrap_or(text.len());
            let mut start = line_of(text, header.clone()).start;
            // With the blank line Brigadier put before it, when it is the last table.
            if end == text.len() && text[..start].ends_with("\n\n") {
                start -= 1;
            }
            if text[line.clone()].trim() != ours {
                return Err(invalid("the trust line was changed"));
            }
            format!("{}{}", &text[..start], &text[end..])
        }
        TrustBefore::NoEntry | TrustBefore::NoKey => {
            if text[line.clone()].trim() != ours {
                return Err(invalid("the trust line was changed"));
            }
            format!("{}{}", &text[..line.start], &text[line.end..])
        }
    };
    parse(&new)?;
    Ok(Some(new))
}

/// The whole line around `range`, with its newline.
fn line_of(text: &str, range: Range<usize>) -> Range<usize> {
    let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |i| range.end + i + 1);
    start..end
}

/// Where the first table header at or after `after` starts (an implicit table, such as
/// `projects` in `[projects."…"]`, has no header of its own).
fn next_table_start(root: &Table, after: usize) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut stack = vec![root];
    while let Some(table) = stack.pop() {
        for (_, item) in table.iter() {
            let tables: Vec<&Table> = match item {
                Item::Table(table) => vec![table],
                Item::ArrayOfTables(array) => array.iter().collect(),
                _ => Vec::new(),
            };
            for table in tables {
                if let Some(span) = table.span()
                    && !table.is_implicit()
                    && span.start >= after
                    && best.is_none_or(|best| span.start < best)
                {
                    best = Some(span.start);
                }
                stack.push(table);
            }
        }
    }
    best
}

/// What [`trust_codex_hook`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookTrust {
    Trusted,
    /// Trusted with that hash already: nothing changed.
    AlreadyTrusted,
}

/// Trusts one of Brigadier's own Codex hooks in Codex's `config.toml` at `file` (harness D49):
/// `[hooks.state."<key>"] trusted_hash = "<hash>"`, the entry Codex itself writes when the
/// user trusts a hook. Only that one key changes; the user's own hooks, their trust and the
/// rest of the file stay byte for byte, and a rerun with the same hash changes nothing. An
/// `enabled` the user set beside it stays theirs.
pub fn trust_codex_hook(file: &Path, key: &str, hash: &str) -> Result<HookTrust> {
    edit(file, LOCK, |text| {
        let state = hook_state(text, key, hash)?;
        if matches!(state, State::Trusted) {
            return Ok(Edit::Keep(HookTrust::AlreadyTrusted));
        }
        let new = trust_hook(text, key, hash, &state)?;
        Ok(Edit::Replace(new, HookTrust::Trusted))
    })
}
