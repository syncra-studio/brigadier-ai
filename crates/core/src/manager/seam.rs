//! The seam (G12, S22): a long session goes on in a fresh CLI session, never compacted. Past
//! `BRIGADIER_SEAM_TOKENS` of hot context (the CLI's own usage events), Brigadier sends the one
//! message it ever pushes into a thread: finish the step you are on, update the ledger, write
//! the hand-off note. Once that turn ends the CLI closes, the thread shows a divider and the
//! successor starts at once (user, 10-23); its first lines are the hand-off (the bootstrap): the
//! map, the whole ledger, the note, the session's first message, the latest exchange and a
//! newest-first verbatim tail, with the exact read command of every span left out (§9.5 L2).
//!
//! `record/seam.json` is the seam's truth across restarts: the CLI generation, where the seam
//! stands ([`Phase`]) and every completed seam. Each phase is written before what it leads to
//! happens, so a restart at any point goes on from the file and never sends a message twice
//! or starts two successors ([`SessionManager::recover_seam`]).

use std::path::{Path, PathBuf};
use std::time::Duration;

use brigadier_providers::session_file::FileEntry;
use brigadier_providers::{ProviderEvent, ProviderKind, Role};
use serde::{Deserialize, Serialize};

use super::records::{READ_MAX, entry_text};
use super::session_dir::{LineageEntry, write_atomic};
use crate::thread_view::{HANDOFF_END, HANDOFF_MARK, NUDGE_MARK, SeamMark};
use crate::{Error, Result};

/// The hot context the seam comes at, in tokens: an env default, never a cap (Q1).
pub(crate) const TOKENS_ENV: &str = "BRIGADIER_SEAM_TOKENS";
/// What the successor is told after its hand-off when no message of the user's waits for it: it
/// starts at once and carries on by itself, as a Delegator's successor does (user, 10-23).
pub(crate) const CARRY_ON: &str = "Carry on from the hand-off note.";
const DEFAULT_TOKENS: i64 = 200_000;
/// How long a user message waits for the note turn before it goes to the successor anyway.
const HOLD: Duration = Duration::from_secs(60);
const SEAM_FILE: &str = "seam.json";
/// The verbatim tail's budget, ≈15k tokens.
const TAIL_BYTES: usize = 60_000;
/// One tail item's most; the rest is a read away.
const ITEM_BYTES: usize = 6_000;
/// One tool item's most in the latest exchange (its messages go whole).
const LATEST_TOOL_BYTES: usize = 4_000;

/// What the seam runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SeamConfig {
    pub tokens: i64,
    pub hold: Duration,
}

impl SeamConfig {
    pub(crate) fn from_env() -> Self {
        Self {
            tokens: std::env::var(TOKENS_ENV)
                .ok()
                .and_then(|value| value.trim().parse::<i64>().ok())
                .filter(|tokens| *tokens > 0)
                .unwrap_or(DEFAULT_TOKENS),
            hold: HOLD,
        }
    }

    /// The context the seam comes at for a model of `window` tokens: the setting, or four
    /// fifths of a smaller window, so the note turn still fits. A generation that started at
    /// `base` grows a quarter of the setting first, so a successor whose hand-off alone comes
    /// near the setting isn't asked for its own at once. Stay below Codex's forced compaction
    /// at nine tenths, even when its reported window has no reserved portion (S26).
    pub(crate) fn threshold(&self, window: Option<i64>, base: Option<i64>) -> i64 {
        let window = window.filter(|window| *window > 0);
        let mut at = match window {
            Some(window) => self.tokens.min(window / 5 * 4),
            None => self.tokens,
        };
        if let Some(base) = base {
            at = at.max(base + self.tokens / 4);
            if let Some(window) = window {
                at = at.min(window / 100 * 85);
            }
        }
        at
    }
}

/// `record/seam.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeamFile {
    /// The CLI generation now: 1 for the session's first, one more at each seam. Seam `n`
    /// ends generation `n`; its note is `handoff/<n>.md`.
    pub generation: u32,
    pub phase: Phase,
    /// Every completed seam, oldest first.
    #[serde(default)]
    pub seams: Vec<Seam>,
}

impl Default for SeamFile {
    fn default() -> Self {
        Self {
            generation: 1,
            phase: Phase::Live,
            seams: Vec::new(),
        }
    }
}

/// Where the seam of the current generation stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub(crate) enum Phase {
    /// No seam under way.
    Live,
    /// The thread was asked for its hand-off note. Messages wait for the successor.
    #[serde(rename_all = "camelCase")]
    Nudged {
        at_ms: i64,
        tokens: i64,
        /// The CLI session asked.
        predecessor: Option<String>,
    },
    /// The predecessor is done (with its note, or dead without one); the next message
    /// starts the successor.
    #[serde(rename_all = "camelCase")]
    Closed { end: SeamEnd },
    /// The successor is being started: its CLI session id, reserved before it exists, and
    /// the messages its first input carries. Recovery asks that session whether it took its
    /// hand-off before anything is sent again.
    #[serde(rename_all = "camelCase")]
    Starting {
        end: SeamEnd,
        successor: String,
        provider: ProviderKind,
        messages: Vec<String>,
    },
}

/// How a generation ended at its seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeamEnd {
    pub predecessor: Option<String>,
    /// When it was asked for its note; none when it died first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudged_at_ms: Option<i64>,
    pub closed_at_ms: i64,
    /// Why it ended without a note; none when it wrote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub death: Option<String>,
}

/// A completed seam.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Seam {
    /// The generation it ended.
    pub n: u32,
    #[serde(flatten)]
    pub end: SeamEnd,
    pub successor: String,
    pub provider: ProviderKind,
    pub started_at_ms: i64,
    /// The note, when it was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The hand-off the successor started from.
    pub start: String,
    /// The messages its first input carried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<String>,
    /// The context the successor's CLI first reported: its generation grows from there
    /// before its own seam ([`SeamConfig::threshold`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_context: Option<i64>,
}

pub(crate) fn read_seam(record: &Path) -> SeamFile {
    std::fs::read(record.join(SEAM_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn write_seam(record: &Path, file: &SeamFile) -> Result<()> {
    std::fs::create_dir_all(record).map_err(|err| Error::Invalid(err.to_string()))?;
    let text = serde_json::to_string_pretty(file).map_err(|err| Error::Invalid(err.to_string()))?;
    write_atomic(&record.join(SEAM_FILE), text.as_bytes())
}

/// Seam `n`'s note.
pub(crate) fn note_path(record: &Path, n: u32) -> PathBuf {
    record
        .join(super::records::HANDOFF_DIR)
        .join(format!("{n}.md"))
}

/// The hand-off seam `n`'s successor starts from.
pub(crate) fn start_path(record: &Path, n: u32) -> PathBuf {
    record
        .join(super::records::HANDOFF_DIR)
        .join(format!("{n}-start.md"))
}

/// The one message of seam `note`'s (§6's seam text, first part).
pub(crate) fn nudge(note: &Path) -> String {
    format!(
        "{NUDGE_MARK} Finish the step you are on, at a boundary you choose; don't start new \
         work.\nUpdate the ledger, then write the hand-off note at {}: doing now, waiting for, \
         next, \"read A,B,C; next D,E\", stale ledger entries. Then stop.\nIf this is your \
         second hand-off in a row that is still reading, say so in the note and propose \
         splitting the work.",
        note.display()
    )
}

/// The first line of seam `n`'s hand-off in session `key`: what recovery looks for in the
/// successor's session.
pub(crate) fn handoff_line(n: u32, key: &str) -> String {
    format!("{HANDOFF_MARK}{n} · {key}]")
}

/// §6's successor lines.
const SUCCESSOR: &str = "You continue an earlier session. The ledger, note and last exchanges \
below are authoritative. For anything else from before, search the record (`brigadier brain \
search`, `brigadier brain read`) and quote what you find; if it isn't there, say so rather than \
guess.";

/// What a successor is told when its predecessor wrote no note.
fn death_line(why: &str) -> String {
    format!(
        "The previous session ended without a hand-off note ({why}): check `git status` and \
         `git log` first."
    )
}

/// What a successor's hand-off is made of.
pub(crate) struct Boot<'a> {
    /// The seam.
    pub n: u32,
    /// How `brigadier brain read` names the session.
    pub key: &'a str,
    pub folder: &'a Path,
    pub ledger_path: &'a Path,
    pub ledger: Option<&'a str>,
    pub lineage: &'a [LineageEntry],
    pub note_path: &'a Path,
    pub note: Option<&'a str>,
    pub start_path: &'a Path,
    pub death: Option<&'a str>,
    /// The session's transcript, every generation, as `brigadier brain read` numbers it.
    pub entries: &'a [FileEntry],
    pub blobs: &'a Path,
}

/// Seam `n`'s hand-off, as the successor's first lines (§9.5 L2): the successor lines, the
/// map, the whole ledger, the note, the session's first user message, the latest exchange in
/// full, the tail newest first, and one read command per span left out.
pub(crate) fn bootstrap(boot: &Boot<'_>) -> String {
    let readable: Vec<Option<(&'static str, String)>> = boot
        .entries
        .iter()
        .map(|entry| entry_text(entry, boot.blobs).map(|(role, text, _)| (role, text)))
        .collect();
    // Brigadier's own nudge is no part of the work: never quoted, never left out.
    let mut shown: Vec<bool> = readable
        .iter()
        .map(|item| {
            item.as_ref()
                .is_some_and(|(_, text)| text.starts_with(NUDGE_MARK))
        })
        .collect();
    let read = |from: usize, to: usize| format!("`brigadier brain read {} {from} {to}`", boot.key);
    let item = |at: usize, max: usize| -> Option<String> {
        let (role, text) = readable[at].as_ref()?;
        let text = without_handoff(text);
        let mut out = format!("[#{at} · {role}]\n");
        if text.len() > max {
            let mut cut = max;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            out.push_str(&text[..cut]);
            out.push_str(&format!(
                "\n[cut at {cut} of {} bytes; {} gives it whole]",
                text.len(),
                read(at, at)
            ));
        } else {
            out.push_str(text);
        }
        Some(out)
    };
    let users: Vec<usize> = boot
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| user_text(entry).is_some_and(|text| !text.starts_with(NUDGE_MARK)))
        .map(|(at, _)| at)
        .collect();

    let mut out = format!("{}\n{SUCCESSOR}\n", handoff_line(boot.n, boot.key));
    if let Some(why) = boot.death {
        out.push_str(&death_line(why));
        out.push('\n');
    }

    out.push_str("\n## Map\n");
    out.push_str(&format!("- Session folder: {}\n", boot.folder.display()));
    out.push_str(&format!(
        "- Ledger: {} (whole below)\n",
        boot.ledger_path.display()
    ));
    match (boot.note, boot.death) {
        (Some(_), _) => out.push_str(&format!(
            "- Hand-off note: {} (below)\n",
            boot.note_path.display()
        )),
        (None, Some(why)) => out.push_str(&format!("- Hand-off note: none ({why})\n")),
        (None, None) => out.push_str("- Hand-off note: none\n"),
    }
    out.push_str(&format!("- This hand-off: {}\n", boot.start_path.display()));
    out.push_str("- Earlier CLI sessions, oldest first:\n");
    for session in boot.lineage {
        out.push_str(&format!(
            "  - {} {}{}\n",
            session.provider.label(),
            session.session_id,
            session
                .file
                .as_deref()
                .map(|file| format!(" ({file})"))
                .unwrap_or_default()
        ));
    }
    if !boot.entries.is_empty() {
        out.push_str(&format!(
            "- Their transcript is #0–#{}: {} reads a span (at most {READ_MAX} items), \
             `brigadier brain search <words>` searches it.\n",
            boot.entries.len() - 1,
            read(0, 0).replace(" 0 0`", " <from> <to>`")
        ));
    }

    out.push_str("\n## Ledger\n");
    match boot.ledger.map(str::trim).filter(|text| !text.is_empty()) {
        Some(ledger) => out.push_str(ledger),
        None => out.push_str("(no ledger yet)"),
    }
    out.push('\n');

    if let Some(note) = boot.note {
        out.push_str("\n## Hand-off note\n");
        out.push_str(note.trim());
        out.push('\n');
    }

    // The session's first user message, always.
    if let Some(&first) = users.first()
        && let Some(text) = item(first, usize::MAX)
    {
        out.push_str("\n## The session's first message\n");
        out.push_str(&text);
        out.push('\n');
        shown[first] = true;
    }

    // The latest exchange, in order: its messages whole, its tool items cut.
    let latest = users.last().copied().filter(|at| !shown[*at]);
    let mut budget = TAIL_BYTES;
    if let Some(from) = latest {
        out.push_str("\n## The latest exchange\n");
        for (at, entry) in boot.entries.iter().enumerate().skip(from) {
            let message = matches!(entry.event, ProviderEvent::Message { .. });
            if entry.parent.is_some() || shown[at] {
                continue;
            }
            let max = if message {
                usize::MAX
            } else {
                LATEST_TOOL_BYTES
            };
            if !message && budget < LATEST_TOOL_BYTES {
                continue;
            }
            if let Some(text) = item(at, max) {
                budget = budget.saturating_sub(text.len());
                out.push_str(&text);
                out.push_str("\n\n");
                shown[at] = true;
            }
        }
    }

    // Before it, newest first, while the budget lasts.
    let before = latest.unwrap_or(boot.entries.len());
    let mut tail = String::new();
    for at in (0..before).rev() {
        if shown[at] || boot.entries[at].parent.is_some() {
            continue;
        }
        let Some(text) = item(at, ITEM_BYTES) else {
            continue;
        };
        if text.len() > budget {
            break;
        }
        budget -= text.len();
        tail.push_str(&text);
        tail.push_str("\n\n");
        shown[at] = true;
    }
    if !tail.is_empty() {
        out.push_str("\n## Earlier, newest first\n");
        out.push_str(&tail);
    }

    // Every span left out, with its read.
    let spans = left_out(&readable, &shown);
    if !spans.is_empty() {
        out.push_str("\n## Left out, each a read away\n");
        for (from, to) in spans {
            out.push_str(&format!("- #{from}–#{to}: {}\n", read(from, to)));
        }
    }
    out.push_str(&format!("\n{HANDOFF_END}"));
    out
}

/// The spans of readable items not shown, each at most [`READ_MAX`] positions long so its
/// read returns all of it.
fn left_out(readable: &[Option<(&'static str, String)>], shown: &[bool]) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    for at in 0..readable.len() {
        if shown[at] {
            spans.extend(open.take());
            continue;
        }
        if readable[at].is_none() {
            continue;
        }
        open = match open {
            Some((from, _)) if at - from < READ_MAX as usize => Some((from, at)),
            Some(span) => {
                spans.push(span);
                Some((at, at))
            }
            None => Some((at, at)),
        };
    }
    spans.extend(open);
    spans
}

/// A user message's text in the session's own thread.
fn user_text(entry: &FileEntry) -> Option<&str> {
    match &entry.event {
        ProviderEvent::Message {
            role: Role::User,
            text,
            ..
        } if entry.parent.is_none() && !entry.steer => Some(text),
        _ => None,
    }
}

/// The thread's seams, each where its divider goes, from `record`'s `seam.json` and the
/// thread's `entries`; a successor's first message loses its hand-off there (it opens from the
/// divider).
pub(crate) fn marks(record: &Path, entries: &mut [FileEntry]) -> Vec<SeamMark> {
    let file = read_seam(record);
    let existing = |path: PathBuf| path.is_file().then(|| path.display().to_string());
    let mut marks: Vec<SeamMark> = file
        .seams
        .iter()
        .map(|seam| SeamMark {
            n: seam.n,
            open: false,
            at: None,
            carried: false,
            nudge: None,
            note: existing(note_path(record, seam.n)),
            start: existing(PathBuf::from(&seam.start)),
            death: seam.end.death.clone().filter(|_| seam.note.is_none()),
        })
        .collect();
    match &file.phase {
        Phase::Live => {}
        Phase::Nudged { .. } => marks.push(SeamMark {
            n: file.generation,
            open: true,
            at: None,
            carried: false,
            nudge: None,
            note: None,
            start: None,
            death: None,
        }),
        Phase::Closed { end } | Phase::Starting { end, .. } => marks.push(SeamMark {
            n: file.generation,
            open: false,
            at: None,
            carried: false,
            nudge: None,
            note: existing(note_path(record, file.generation)),
            start: None,
            death: end.death.clone(),
        }),
    }
    // Each nudge goes with the next seam: the one its successor starts after.
    let mut nudge = None;
    for entry in entries.iter_mut() {
        let ProviderEvent::Message {
            item_id,
            role: Role::User,
            text,
        } = &mut entry.event
        else {
            continue;
        };
        if text.starts_with(NUDGE_MARK) {
            nudge = Some(item_id.clone());
            continue;
        }
        let Some(rest) = text.strip_prefix(HANDOFF_MARK) else {
            continue;
        };
        let n = rest
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|n| n.parse::<u32>().ok());
        let own = without_handoff(text).to_owned();
        *text = own;
        if let Some(mark) = marks.iter_mut().find(|mark| Some(mark.n) == n) {
            mark.carried = text == CARRY_ON;
            mark.at = Some(item_id.clone());
            mark.nudge = nudge.take();
        }
    }
    // The seam under way, or closed with its successor still to come.
    if let Some(pending) = marks.last_mut().filter(|mark| mark.at.is_none()) {
        pending.nudge = nudge;
    }
    marks
}

/// `text` without the hand-off a successor's first message starts with.
pub(crate) fn without_handoff(text: &str) -> &str {
    match text.strip_prefix(HANDOFF_MARK) {
        Some(rest) => rest
            .find(HANDOFF_END)
            .map_or(text, |end| rest[end + HANDOFF_END.len()..].trim_start()),
        None => text,
    }
}

#[cfg(test)]
mod tests;
