//! The caller's kept ledger, writable through `brigadier` even in a Chat (S17), with S22b's
//! plumbing on top (`research/CONTINUITY.md` §3.1–§3.5): `add` stamps each entry
//! `L<n> · <MM-DD HH:MM> · gen <g> ·` (ids count up per ledger, its archive included, and are
//! never reused; the model never types the time), `close` appends a closing entry, `show
//! --open|--rules` filters by kind, and `check` lists what a derived file leaves out. Nothing
//! is ever rewritten and nothing is judged: the thread reads the lists and decides.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use brigadier_cli::protocol::{LedgerCheck, LedgerEnded, LedgerEntry, LedgerNoId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::SessionManager;
use super::cli::CliCaller;
use super::records::LEDGER_ARCHIVE_FILE;
use super::session_dir::LEDGER_FILE;
use crate::{Error, Result};

impl SessionManager {
    pub(super) async fn cli_ledger(
        &self,
        caller: &CliCaller,
        action: &str,
        given: Value,
    ) -> Result<(String, Option<LedgerCheck>)> {
        // Check the stored conversation too: a tab chat must never leave a kept record.
        if self
            .core
            .conversation(&caller.conversation_id)?
            .tab_chat
            .is_some()
        {
            return Err(Error::Invalid(
                "a chat beside a tab cannot keep a ledger".into(),
            ));
        }
        let folder = self.session_dir(&caller.conversation_id);
        let record = match &caller.task_id {
            Some(id) => {
                let task = self.task_by_id(&caller.conversation_id, id).await?;
                folder.worker(&task).join("record")
            }
            None => folder.record(),
        };
        let invalid = |err: serde_json::Error| Error::Invalid(format!("bad ledger call: {err}"));
        match action {
            "add" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Add {
                    text: String,
                }
                let add: Add = serde_json::from_value(given).map_err(invalid)?;
                super::blocking(move || add_entry(&record, &add.text, Stamp::now(&record)))
                    .await
                    .map(|text| (text, None))
            }
            "close" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Close {
                    id: String,
                    how: String,
                }
                let close: Close = serde_json::from_value(given).map_err(invalid)?;
                super::blocking(move || {
                    close_entry(&record, &close.id, &close.how, Stamp::now(&record))
                })
                .await
                .map(|text| (text, None))
            }
            "show" => {
                #[derive(Deserialize, Default)]
                #[serde(deny_unknown_fields)]
                struct Show {
                    #[serde(default)]
                    open: bool,
                    #[serde(default)]
                    rules: bool,
                }
                let show: Show = match given {
                    Value::Null => Show::default(),
                    given => serde_json::from_value(given).map_err(invalid)?,
                };
                super::blocking(move || show_entries(&record, show.open, show.rules))
                    .await
                    .map(|text| (text, None))
            }
            "check" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct CheckFile {
                    file: String,
                    text: String,
                }
                let given: CheckFile = serde_json::from_value(given).map_err(invalid)?;
                super::blocking(move || {
                    let check = check_file(&record, &given.file, &given.text)?;
                    Ok((render(&check), Some(check)))
                })
                .await
            }
            other => Err(Error::Invalid(format!("no ledger action {other:?}"))),
        }
    }
}

/// What `add` writes before an entry: its time and the CLI generation it was written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stamp {
    /// `MM-DD HH:MM`, local time.
    pub at: String,
    pub generation: u32,
}

impl Stamp {
    fn now(record: &Path) -> Self {
        Self {
            at: jiff::Zoned::now().strftime("%m-%d %H:%M").to_string(),
            generation: super::seam::read_seam(record).generation,
        }
    }
}

/// Which of the record's two ledger files an entry is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum LedgerFile {
    Archive,
    Ledger,
}

impl LedgerFile {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Archive => LEDGER_ARCHIVE_FILE,
            Self::Ledger => LEDGER_FILE,
        }
    }
}

/// An entry's kind, read from the words its text starts with (§6's kinds).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Kind {
    Rule,
    Decision,
    Open,
    /// `ledger close`'s entry, and the ids it closes.
    Close(Vec<u32>),
    Other,
}

/// One ledger entry as written: a list item with its indented lines, or a line of prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub file: LedgerFile,
    /// Its first and last line in its file, from 1.
    pub line: usize,
    pub last_line: usize,
    /// Verbatim, every line of it.
    pub text: String,
    pub id: Option<u32>,
    pub kind: Kind,
    /// The ids it says it supersedes.
    pub supersedes: Vec<u32>,
}

impl Entry {
    /// A rule, a decision or an open item: what travels whole across every seam.
    fn pinned_kind(&self) -> bool {
        matches!(self.kind, Kind::Rule | Kind::Decision | Kind::Open)
    }
}

/// How an entry stopped applying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ended {
    Closed { by: Option<u32> },
    Superseded { by: Option<u32> },
}

/// A session's ledger and its archive, parsed: the archive's entries first.
#[derive(Debug, Clone, Default)]
pub(crate) struct Ledger {
    pub entries: Vec<Entry>,
    ended: HashMap<u32, Ended>,
}

impl Ledger {
    pub(crate) fn parse(archive: &str, ledger: &str) -> Self {
        let mut entries = parse_entries(archive, LedgerFile::Archive);
        entries.extend(parse_entries(ledger, LedgerFile::Ledger));
        let mut ended = HashMap::new();
        for entry in &entries {
            if let Kind::Close(ids) = &entry.kind {
                for id in ids {
                    ended.insert(*id, Ended::Closed { by: entry.id });
                }
            }
            for id in &entry.supersedes {
                ended
                    .entry(*id)
                    .or_insert(Ended::Superseded { by: entry.id });
            }
        }
        Self { entries, ended }
    }

    pub(crate) fn read(record: &Path) -> Result<Self> {
        let archive = read(&record.join(LEDGER_ARCHIVE_FILE))?;
        let ledger = read(&record.join(LEDGER_FILE))?;
        Ok(Self::parse(&archive, &ledger))
    }

    /// The id the next entry gets: one past the highest either file holds.
    pub(crate) fn next_id(&self) -> u32 {
        self.entries
            .iter()
            .filter_map(|entry| entry.id)
            .max()
            .unwrap_or(0)
            + 1
    }

    pub(crate) fn ended(&self, id: u32) -> Option<&Ended> {
        self.ended.get(&id)
    }

    fn live(&self, entry: &Entry) -> bool {
        entry.id.is_none_or(|id| !self.ended.contains_key(&id))
    }

    /// Open items not closed.
    pub(crate) fn open(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|entry| entry.kind == Kind::Open && self.live(entry))
            .collect()
    }

    /// Rules and decisions neither closed nor superseded.
    pub(crate) fn rules(&self) -> Vec<&Entry> {
        self.entries
            .iter()
            .filter(|entry| matches!(entry.kind, Kind::Rule | Kind::Decision) && self.live(entry))
            .collect()
    }

    /// Whether `entry` travels whole across a seam: an open item, rule or decision still
    /// standing.
    pub(crate) fn pinned(&self, entry: &Entry) -> bool {
        entry.pinned_kind() && self.live(entry)
    }

    fn find(&self, id: u32) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == Some(id))
    }
}

/// The entries of one ledger file. A list item runs on through its indented lines; any other
/// line that isn't blank or a heading is an entry of its own.
pub(crate) fn parse_entries(text: &str, file: LedgerFile) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut open = false;
    for (at, line) in text.lines().enumerate() {
        let number = at + 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            open = false;
            continue;
        }
        let indented = line.starts_with([' ', '\t']);
        if open
            && indented
            && list_body(line.trim_start()).is_none()
            && let Some(last) = entries.last_mut()
        {
            last.text.push('\n');
            last.text.push_str(line);
            last.last_line = number;
            continue;
        }
        let body = list_body(line.trim_start()).unwrap_or(line.trim());
        let (id, body) = split_stamp(body);
        open = list_body(line.trim_start()).is_some();
        entries.push(Entry {
            file,
            line: number,
            last_line: number,
            text: line.to_owned(),
            id,
            kind: kind_of(body),
            supersedes: supersedes(body),
        });
    }
    entries
}

const MARKERS: [&str; 3] = ["- ", "* ", "+ "];

fn list_body(line: &str) -> Option<&str> {
    MARKERS.iter().find_map(|marker| line.strip_prefix(marker))
}

/// An entry's id and its text after the stamp: `L14 · 10-23 11:40 · gen 2 · text`.
fn split_stamp(body: &str) -> (Option<u32>, &str) {
    let Some((head, rest)) = body.split_once(" · ") else {
        return (None, body);
    };
    let Some(id) = id_of(head) else {
        return (None, body);
    };
    // The time and generation, when `add` wrote them.
    let mut rest = rest;
    if let Some((time, after)) = rest.split_once(" · ")
        && time.len() == 11
        && time.as_bytes()[2] == b'-'
    {
        rest = after;
    }
    if let Some((generation, after)) = rest.split_once(" · ")
        && generation.starts_with("gen ")
    {
        rest = after;
    }
    (Some(id), rest)
}

/// `L14` → 14.
fn id_of(word: &str) -> Option<u32> {
    let digits = word.strip_prefix('L')?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

fn kind_of(body: &str) -> Kind {
    let head = body
        .trim_start_matches(|c: char| matches!(c, '*' | '_' | '[' | '`') || c.is_whitespace())
        .to_lowercase();
    // The whole word ("Ruled out" is no rule, "Opened" no open item), or its plural.
    let word = |word: &str| {
        head.strip_prefix(word).is_some_and(|rest| {
            let rest = rest.strip_prefix('s').unwrap_or(rest);
            !rest.starts_with(|c: char| c.is_alphanumeric())
        })
    };
    if word("rule") {
        Kind::Rule
    } else if word("decision") || word("user decided") || word("i decided") {
        Kind::Decision
    } else if word("open") {
        Kind::Open
    } else if word("closed") || word("close") {
        let clause = body.split(':').next().unwrap_or(body);
        Kind::Close(cited(clause))
    } else {
        Kind::Other
    }
}

/// The ids named right after "supersedes".
fn supersedes(body: &str) -> Vec<u32> {
    let lower = body.to_lowercase();
    let mut ids = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find("supersedes") {
        let after = from + at + "supersedes".len();
        for word in body[after..].split_whitespace() {
            let word = word.trim_matches(|c: char| ",.;:()\"'`".contains(c));
            match id_of(word) {
                Some(id) => ids.push(id),
                None if matches!(word, "and" | "&" | "") => {}
                None => break,
            }
        }
        from = after;
    }
    ids
}

/// Every `L<n>` id `text` names, in order, each once.
pub(crate) fn cited(text: &str) -> Vec<u32> {
    let bytes = text.as_bytes();
    let mut ids = Vec::new();
    for (at, _) in text.match_indices('L') {
        let before = at.checked_sub(1).map(|i| bytes[i]);
        if before.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
            continue;
        }
        let end = bytes[at + 1..]
            .iter()
            .position(|b| !b.is_ascii_digit())
            .map_or(bytes.len(), |n| at + 1 + n);
        if end == at + 1
            || bytes
                .get(end)
                .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            continue;
        }
        if let Ok(id) = text[at + 1..end].parse::<u32>()
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

fn read(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(Error::Invalid(format!("couldn't read the ledger: {err}"))),
    }
}

/// One append at a time, including when a session runs commands concurrently, so two entries
/// never get one id.
static APPENDS: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// `ledger add`: the entry, stamped with the next id; prints its id.
pub(crate) fn add_entry(record: &Path, text: &str, stamp: Stamp) -> Result<String> {
    let _one = APPENDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let text = text.trim();
    let text = list_body(text).unwrap_or(text);
    let text = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return Err(Error::Invalid(
            "give a ledger entry, with its kind and source".into(),
        ));
    }
    append(record, &text, &stamp)
}

/// `ledger close <id> "<how>"`: a closing entry, appended; nothing is rewritten.
pub(crate) fn close_entry(record: &Path, id: &str, how: &str, stamp: Stamp) -> Result<String> {
    let _one = APPENDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(id) = id_of(id.trim()) else {
        return Err(Error::Invalid(format!(
            "{id:?} is no entry id; ids look like L12"
        )));
    };
    let how = how.split_whitespace().collect::<Vec<_>>().join(" ");
    if how.is_empty() {
        return Err(Error::Invalid("say how it was closed".into()));
    }
    if Ledger::read(record)?.find(id).is_none() {
        return Err(Error::Invalid(format!(
            "the ledger has no entry L{id}; `brigadier ledger show --open` lists the open ones"
        )));
    }
    append(record, &format!("Closed L{id}: {how}"), &stamp)
}

/// Appends `text` as the next entry, under the caller's lock.
fn append(record: &Path, text: &str, stamp: &Stamp) -> Result<String> {
    let path = record.join(LEDGER_FILE);
    let id = Ledger::read(record)?.next_id();
    let old = read(&path)?;
    let marker = old
        .lines()
        .rev()
        .find_map(|line| MARKERS.into_iter().find(|marker| line.starts_with(marker)))
        .unwrap_or("- ");
    let newline = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let separator = if old.is_empty() || old.ends_with('\n') {
        ""
    } else {
        newline
    };
    let entry = format!(
        "{separator}{marker}L{id} · {} · gen {} · {text}{newline}",
        stamp.at, stamp.generation
    );
    let failed = |err| Error::Invalid(format!("couldn't append to the ledger: {err}"));
    std::fs::create_dir_all(record).map_err(failed)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(entry.as_bytes()))
        .map_err(failed)?;
    Ok(format!("L{id}\n"))
}

/// `ledger show`: everything verbatim, or with `--open` and `--rules` only those, whole.
pub(crate) fn show_entries(record: &Path, open: bool, rules: bool) -> Result<String> {
    if !open && !rules {
        return read(&record.join(LEDGER_FILE));
    }
    let ledger = Ledger::read(record)?;
    let mut out = String::new();
    let mut section = |title: &str, entries: Vec<&Entry>| {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("{title} ({}):\n", entries.len()));
        for entry in entries {
            out.push_str(&entry.text);
            out.push('\n');
        }
    };
    if open {
        section("Open items", ledger.open());
    }
    if rules {
        section("Rules and decisions", ledger.rules());
    }
    Ok(out)
}

pub(crate) fn check_file(record: &Path, file: &str, text: &str) -> Result<LedgerCheck> {
    Ok(check(&Ledger::read(record)?, file, text))
}

pub(crate) fn check(ledger: &Ledger, file: &str, text: &str) -> LedgerCheck {
    let cites = cited(text);
    let uncited = ledger
        .rules()
        .into_iter()
        .filter_map(|entry| {
            let id = entry.id?;
            (!cites.contains(&id)).then(|| LedgerEntry {
                id: format!("L{id}"),
                entry: entry.text.clone(),
            })
        })
        .collect();
    let mut ended = Vec::new();
    let mut unknown = Vec::new();
    for id in cites {
        match (ledger.find(id), ledger.ended(id)) {
            (None, _) => unknown.push(format!("L{id}")),
            (Some(_), Some(how)) => {
                let (how, by) = match how {
                    Ended::Closed { by } => ("closed", by),
                    Ended::Superseded { by } => ("superseded", by),
                };
                ended.push(LedgerEnded {
                    id: format!("L{id}"),
                    how: how.to_owned(),
                    by: by.map(|by| format!("L{by}")),
                });
            }
            (Some(_), None) => {}
        }
    }
    let no_id = ledger
        .entries
        .iter()
        .filter(|entry| entry.id.is_none())
        .map(|entry| LedgerNoId {
            file: entry.file.name().to_owned(),
            line: u32::try_from(entry.line).unwrap_or(u32::MAX),
            entry: entry.text.clone(),
        })
        .collect();
    LedgerCheck {
        file: file.to_owned(),
        uncited,
        ended,
        unknown,
        no_id,
    }
}

/// `check`'s text: each list that isn't empty, entries verbatim.
pub(crate) fn render(check: &LedgerCheck) -> String {
    let mut out = format!("{} against the ledger:\n", check.file);
    let mut any = false;
    let mut section = |title: &str, lines: Vec<String>| {
        if lines.is_empty() {
            return;
        }
        any = true;
        out.push_str(&format!("\n{title} ({}):\n", lines.len()));
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
    };
    section(
        "Rules and decisions it never cites",
        check
            .uncited
            .iter()
            .map(|cited| cited.entry.clone())
            .collect(),
    );
    section(
        "Ids it cites that are closed or superseded",
        check
            .ended
            .iter()
            .map(|ended| match &ended.by {
                Some(by) => format!("- {} ({} by {by})", ended.id, ended.how),
                None => format!("- {} ({})", ended.id, ended.how),
            })
            .collect(),
    );
    section(
        "Ids it cites that the ledger doesn't have",
        check.unknown.iter().map(|id| format!("- {id}")).collect(),
    );
    section(
        "Ledger entries with no id",
        check
            .no_id
            .iter()
            .map(|entry| format!("- {}:{}: {}", entry.file, entry.line, entry.entry))
            .collect(),
    );
    if !any {
        out.push_str(
            "Nothing to list: it cites every rule and decision, none closed or superseded, \
             and every entry has an id.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests;
