//! The seam (G12, S22): a long session goes on in a fresh CLI session, never compacted. Past
//! `BRIGADIER_SEAM_TOKENS` of hot context (the CLI's own usage events), Brigadier sends the one
//! message it ever pushes into a thread: finish the step you are on, update the ledger, write
//! the hand-off note. Once that turn ends the CLI closes, the thread shows a divider and the
//! successor starts at once (user, 10-23); its first lines are the hand-off (the bootstrap): the
//! note and the pointer to `brigadier context`, where it pulls everything else (D13). A move to
//! another CLI, which has no history of its own, also carries every user message of the lineage,
//! the latest exchange and a newest-first verbatim tail, with the exact read command of
//! everything left out (D49, §9.5 L2), but never the ledger (D13).
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

use super::long_sessions::LongSessions;
use super::records::{READ_MAX, entry_text};
use super::session_dir::{LineageEntry, write_atomic};
use crate::manager::conversation::queue_judging::without_frames;
use crate::thread_view::{HANDOFF_END, HANDOFF_MARK, NUDGE_MARK, SeamMark};
use crate::{Error, Result};

/// The hot context the seam comes at, in tokens: an env default, never a cap (Q1).
pub(crate) const TOKENS_ENV: &str = "BRIGADIER_SEAM_TOKENS";
/// What the successor is told after its hand-off when no message of the user's waits for it: it
/// starts at once and carries on by itself, as a Delegator's successor does (user, 10-23).
pub(crate) const CARRY_ON: &str = "Carry on from the hand-off note.";

/// An end mark quoted inside a hand-off (an earlier one's, in the transcript or the ledger), so
/// the one at its end is the only one.
const QUOTED_END: &str = "[end of an earlier hand-off]";
const DEFAULT_TOKENS: i64 = 200_000;
/// How long a user message waits for the note turn before it goes to the successor anyway.
const HOLD: Duration = Duration::from_secs(60);
const SEAM_FILE: &str = "seam.json";
/// The verbatim tail's budget, ≈15k tokens.
const TAIL_BYTES: usize = 60_000;
/// An expanded tail's most, ≈32k tokens, however large the window: a hand-off stays a start.
const EXPANDED_TAIL_BYTES: usize = 128_000;
/// One tail item's most; the rest is a read away.
const ITEM_BYTES: usize = 6_000;
/// One tool item's most in the latest exchange (its messages go whole).
const LATEST_TOOL_BYTES: usize = 4_000;
/// The user's own messages' budget, ≈20k tokens; the first and the latest go whole regardless.
const USER_BYTES: usize = 80_000;

/// What the seam runs with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SeamConfig {
    pub tokens: i64,
    pub hold: Duration,
    /// `BRIGADIER_LONG_SESSIONS`: the CLI compacts in place instead of size seams
    /// ([`super::long_sessions`]); `None`: off, seams as before.
    pub long: Option<LongSessions>,
    /// [`TOKENS_ENV`] was set: a long session's Codex compacts there too, the point a shared
    /// comparison asks for, instead of at Codex's own (run2-D6).
    pub tokens_set: bool,
}

impl SeamConfig {
    pub(crate) fn from_env() -> Self {
        let set = std::env::var(TOKENS_ENV)
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .filter(|tokens| *tokens > 0);
        Self {
            tokens: set.unwrap_or(DEFAULT_TOKENS),
            hold: HOLD,
            long: LongSessions::from_env(),
            tokens_set: set.is_some(),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SeamFile {
    /// The CLI generation now: 1 for the session's first, one more at each seam. Seam `n`
    /// ends generation `n`; its note is `handoff/<n>.md`.
    pub generation: u32,
    pub phase: Phase,
    /// Every completed seam, oldest first.
    #[serde(default)]
    pub seams: Vec<Seam>,
    /// The other CLI the session was asked to continue on (S38c, D46): the seam comes once no
    /// turn runs, and its successor starts there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moving: Option<crate::model::ModelChoice>,
}

impl Default for SeamFile {
    fn default() -> Self {
        Self {
            generation: 1,
            phase: Phase::Live,
            seams: Vec::new(),
            moving: None,
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
    /// The predecessor's last reported hot context (D49), retained across recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hot_tokens: Option<i64>,
    /// When it was asked for its note; none when it died first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nudged_at_ms: Option<i64>,
    pub closed_at_ms: i64,
    /// Why it ended without a note; none when it wrote one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub death: Option<String>,
    /// The user stopped its last turn: the successor waits for their next message.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// Messages the thread already shows that the successor is given again (a usage limit's
    /// move to another agent, S38): its first message is no bubble of the user's twice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shown: Vec<String>,
    /// The other CLI its successor starts on, as asked (S38c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_to: Option<ProviderKind>,
}

/// The existing seam record distinguishes a requested move, quota fallback and size seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    Size,
    Request,
    Quota,
}

impl SeamEnd {
    pub(crate) fn reason(&self) -> Reason {
        if self.moved_to.is_some() {
            Reason::Request
        } else if self
            .death
            .as_deref()
            .is_some_and(|why| why.starts_with("usage limit on "))
        {
            Reason::Quota
        } else {
            Reason::Size
        }
    }
}

/// D49: only non-size seams can expand. Unknown usage or target windows keep S22's tail.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TailBudget {
    pub reason: Reason,
    pub hot_tokens: Option<i64>,
    pub target_window: Option<i64>,
}

impl TailBudget {
    fn expanded(&self) -> Option<usize> {
        if self.reason == Reason::Size {
            return None;
        }
        let hot = usize::try_from(self.hot_tokens.filter(|n| *n > 0)?).ok()?;
        let window = usize::try_from(self.target_window.filter(|n| *n > 0)?).ok()?;
        // The bootstrap's existing approximation is four UTF-8 bytes per token.
        Some(
            hot.saturating_mul(4)
                .min(window.saturating_mul(2))
                .min(EXPANDED_TAIL_BYTES),
        )
    }

    fn bytes(&self, sections: usize) -> usize {
        self.expanded()
            .map_or(TAIL_BYTES, |cap| cap.saturating_sub(sections))
    }
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

/// Seam `n` is complete: its successor runs, as generation `n + 1`.
pub(crate) fn complete(
    file: &mut SeamFile,
    record: &Path,
    n: u32,
    successor: String,
    provider: ProviderKind,
    end: SeamEnd,
    messages: Vec<String>,
) {
    let note = note_path(record, n);
    file.seams.push(Seam {
        n,
        end,
        successor,
        provider,
        started_at_ms: crate::now_ms(),
        note: note.is_file().then(|| note.display().to_string()),
        start: start_path(record, n).display().to_string(),
        messages,
        first_context: None,
    });
    file.generation = n + 1;
    file.phase = Phase::Live;
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
        "{NUDGE_MARK} This is internal maintenance: do not narrate it or send a hand-off \
         status to the user. Keep the note concise; reference transcript entries instead of \
         copying transcripts or command outputs. Finish the step you are on, at a boundary you \
         choose; don't start new \
         work.\nUpdate the ledger. Run `brigadier ledger show --open --rules`: close or carry \
         each open item by id, and check your last steps kept each rule; write any slip as a \
         Failed check.\nThen write the hand-off note at {}: doing now, waiting for, next, \
         \"read A,B,C; next D,E\", stale ledger entries. Don't restate rules or decisions; the \
         next session reads them with `brigadier context`. Absolute paths only.\nAdd a \"Skills to use next\" section when any \
         apply, naming the skills by the names available to you.\nKeep API keys, passwords, \
         tokens and personal data out of the note; say where they live instead. Then stop.\n\
         If this is your second hand-off in a \
         row that is still reading, say so in the note and propose splitting the work.",
        note.display()
    )
}

/// What a hand-off note adds in a long session (P3 b and e, the default since D15): there a seam
/// is the rare fallback or a move to another CLI (run2-D10), and the successor starts from the
/// working state and the evidence, not from a re-read.
pub(crate) const LONG_NOTE: &str = "\nIn the note also name the files you changed, the last \
test you ran with its exact command and result, any failure still open, and the next step you \
can run. For each failed check and each worker claim you verified, one line: what it said and \
the exact `brigadier brain read <session> --item <id>` that shows it.";

/// Where a successor finds everything about its job (D13): it pulls it, as every session and
/// worker does after a compaction; Brigadier puts none of it in.
pub(crate) const CONTEXT_POINTER: &str = "Everything about your job (brief, decisions, open \
questions, ledger, transcript, skills, MCP servers, project, routing) is in `brigadier context`; \
run it now.";

/// The first line of seam `n`'s hand-off in session `key`: what recovery looks for in the
/// successor's session.
pub(crate) fn handoff_line(n: u32, key: &str) -> String {
    format!("{HANDOFF_MARK}{n} · {key}]")
}

/// §6's successor lines, for the successor that starts generation `generation`; `moved`: on
/// another CLI, given the conversation below.
fn successor_lines(generation: u32, moved: bool) -> String {
    let carried = if moved {
        " on another CLI, which has none of its history: the note and the conversation below \
         are what it carried, and authoritative"
    } else {
        ""
    };
    format!(
        "Resume the user's work without announcing this internal continuation, ledger \
         operations, or hand-off files. You continue an earlier session{carried}.\n\
         {CONTEXT_POINTER} Then add one ledger entry: \"Took over gen {generation} · open <ids> \
         · applies next: <id> (how), …\". If a question waits on the user, raise the oldest one \
         in this reply."
    )
}

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
    pub tail: TailBudget,
    /// How `brigadier brain read` names the session.
    pub key: &'a str,
    pub folder: &'a Path,
    pub lineage: &'a [LineageEntry],
    pub note_path: &'a Path,
    pub note: Option<&'a str>,
    pub start_path: &'a Path,
    pub death: Option<&'a str>,
    /// The session's transcript, every generation, as `brigadier brain read` numbers it.
    pub entries: &'a [FileEntry],
    pub blobs: &'a Path,
}

/// Seam `n`'s hand-off, as the successor's first lines. On the same CLI (a compaction count, a
/// failed or looping compaction, a size seam): the successor lines with the `brigadier context`
/// pointer, and the note, or why there is none (D13, D14, D16). A move to another CLI (a
/// request or a usage limit, D46/D49) has no history of its own there, so it also gets the map,
/// the user's own messages, the latest exchange, the tail newest first and one read command
/// for everything left out (§9.5 L2), but no ledger sections: the decisions and the rest are a
/// `brigadier context` away like everything else (D13).
pub(crate) fn bootstrap(boot: &Boot<'_>) -> String {
    if boot.tail.reason == Reason::Size {
        let mut out = format!(
            "{}\n{}\n",
            handoff_line(boot.n, boot.key),
            successor_lines(boot.n + 1, false)
        );
        match (boot.note, boot.death) {
            (Some(note), _) => {
                out.push_str("\n## Hand-off note\n");
                out.push_str(note.trim());
                out.push('\n');
            }
            (None, why) => {
                out.push_str(&death_line(why.unwrap_or("none written")));
                out.push('\n');
            }
        }
        let mut out = out.replace(HANDOFF_END, QUOTED_END);
        out.push_str(&format!("\n{HANDOFF_END}"));
        return out;
    }
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
                .is_some_and(|(_, text)| text.trim_start().starts_with(NUDGE_MARK))
        })
        .collect();
    let read = |from: usize, to: usize| format!("`brigadier brain read {} {from} {to}`", boot.key);
    let item = |at: usize, max: usize| -> Option<String> {
        let (role, text) = readable[at].as_ref()?;
        // The user's words, without the frame a queued message reached the session under.
        let text = without_frames(without_handoff(text));
        let text = text.as_ref();
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
        .filter(|(_, entry)| {
            user_text(entry).is_some_and(|text| !text.trim_start().starts_with(NUDGE_MARK))
        })
        .map(|(at, _)| at)
        .collect();

    let mut out = format!(
        "{}\n{}\n",
        handoff_line(boot.n, boot.key),
        successor_lines(boot.n + 1, true)
    );
    if let Some(why) = boot.death {
        out.push_str(&death_line(why));
        out.push('\n');
    }

    out.push_str("\n## Map\n");
    out.push_str(&format!("- Session folder: {}\n", boot.folder.display()));
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

    if let Some(note) = boot.note {
        out.push_str("\n## Hand-off note\n");
        out.push_str(note.trim());
        out.push('\n');
    }

    // Every message of the user's, verbatim: the first and the latest always, the rest
    // newest first within the budget, shown oldest first.
    let said: Vec<usize> = boot
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| said_by_user(entry))
        .map(|(at, _)| at)
        .collect();
    if !said.is_empty() {
        let mut kept = vec![false; said.len()];
        let mut budget = USER_BYTES;
        for at in [0, said.len() - 1] {
            if !kept[at]
                && let Some(text) = item(said[at], usize::MAX)
            {
                budget = budget.saturating_sub(text.len());
                kept[at] = true;
            }
        }
        for at in (0..said.len()).rev() {
            if kept[at] {
                continue;
            }
            let Some(text) = item(said[at], usize::MAX) else {
                continue;
            };
            if text.len() > budget {
                break;
            }
            budget -= text.len();
            kept[at] = true;
        }
        out.push_str("\n## The user's messages, oldest first\n");
        for (at, _) in kept.iter().enumerate().filter(|(_, kept)| **kept) {
            if let Some(text) = item(said[at], usize::MAX) {
                out.push_str(&text);
                out.push_str("\n\n");
                shown[said[at]] = true;
            }
        }
        // The fill stops at the first that doesn't fit, so those left out run together.
        let left: Vec<usize> = said
            .iter()
            .zip(&kept)
            .filter(|(_, kept)| !**kept)
            .map(|(at, _)| *at)
            .collect();
        if let (Some(first), Some(last)) = (left.first(), left.last()) {
            out.push_str(&format!(
                "Left out here, for length: {} older messages, #{first}–#{last}; the reads under \
                 \"Left out\" below return them.\n",
                left.len()
            ));
        }
    }

    // The latest exchange, in order: its messages whole, its tool items cut.
    let latest = users.last().copied();
    let mut budget = boot.tail.bytes(out.len());
    let expanded = boot.tail.expanded().is_some();
    if let Some(from) = latest {
        let mut exchange = String::new();
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
            if !message && budget < if expanded { 1 } else { LATEST_TOOL_BYTES } {
                continue;
            }
            if let Some(text) = item(at, max) {
                budget = budget.saturating_sub(text.len());
                exchange.push_str(&text);
                exchange.push_str("\n\n");
                shown[at] = true;
            }
        }
        if !exchange.is_empty() {
            out.push_str(&format!(
                "\n## The latest exchange, after #{from}{}\n",
                if shown[from] { " (above)" } else { "" }
            ));
            out.push_str(&exchange);
        }
    }

    // Before it, newest first, while the budget lasts.
    let before = latest.unwrap_or(boot.entries.len());
    let mut tail = String::new();
    for at in (0..before).rev() {
        if shown[at] || boot.entries[at].parent.is_some() {
            continue;
        }
        let Some(mut text) = item(
            at,
            if expanded {
                budget.min(ITEM_BYTES)
            } else {
                ITEM_BYTES
            },
        ) else {
            continue;
        };
        // An expanded tail cuts the item to what is left, its header and cut line included.
        if expanded && text.len() > budget {
            let over = text.len() - budget;
            if let Some(cut) = item(at, budget.saturating_sub(over)) {
                text = cut;
            }
        }
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
    // One end mark, the last line: one quoted from an earlier hand-off is no end.
    let mut out = out.replace(HANDOFF_END, QUOTED_END);
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

/// Whether `entry` is a message the user wrote, in the thread or mid-turn: never Brigadier's
/// nudge, nor a successor's start with no message of the user's in it.
fn said_by_user(entry: &FileEntry) -> bool {
    match &entry.event {
        ProviderEvent::Message {
            role: Role::User,
            text,
            ..
        } if entry.parent.is_none() && !text.trim_start().starts_with(NUDGE_MARK) => {
            let own = without_handoff(text).trim();
            !own.is_empty() && own != CARRY_ON && !own.starts_with(HANDOFF_MARK)
        }
        _ => false,
    }
}

/// The thread's seams, each where its divider goes, from `record`'s `seam.json` and the
/// thread's `entries`; a successor's first message loses its hand-off there (it opens from the
/// divider).
pub(crate) fn marks(record: &Path, entries: &mut [FileEntry]) -> Vec<SeamMark> {
    let file = read_seam(record);
    let existing = |path: PathBuf| path.is_file().then(|| path.display().to_string());
    // Per mark: its successor's first input carried only messages the thread already shows.
    let again = |end: &SeamEnd, messages: &[String]| {
        !messages.is_empty() && messages.iter().all(|id| end.shown.contains(id))
    };
    let mut shown: Vec<bool> = file
        .seams
        .iter()
        .map(|seam| again(&seam.end, &seam.messages))
        .collect();
    if let Phase::Starting { end, messages, .. } = &file.phase {
        shown.push(again(end, messages));
    }
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
            moved_to: seam.end.moved_to,
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
            moved_to: None,
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
            moved_to: end.moved_to,
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
        if text.trim_start().starts_with(NUDGE_MARK) {
            nudge = Some(item_id.clone());
            continue;
        }
        let Some(rest) = text.trim_start().strip_prefix(HANDOFF_MARK) else {
            continue;
        };
        let n = rest
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|n| n.parse::<u32>().ok());
        let own = without_handoff(text).to_owned();
        *text = own;
        if let Some((mark, shown)) = marks
            .iter_mut()
            .zip(shown.iter().chain(std::iter::repeat(&false)))
            .find(|(mark, _)| Some(mark.n) == n)
        {
            mark.carried = text == CARRY_ON || *shown;
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
    match text.trim_start().strip_prefix(HANDOFF_MARK) {
        Some(rest) => rest
            .find(HANDOFF_END)
            .map_or(text, |end| rest[end + HANDOFF_END.len()..].trim_start()),
        None => text,
    }
}

#[cfg(test)]
mod tests;
