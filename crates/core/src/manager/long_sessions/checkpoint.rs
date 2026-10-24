//! A long session's checkpoint (P2b, run2-D9): its working state at a compaction Brigadier
//! triggers, with exact pointers back to the source, so it can be read again word for word. A
//! separate, replaceable record, never ledger entries (D28: the ledger stays append-only):
//! `record/checkpoint.md` is the current one, `record/checkpoints.jsonl` keeps every version
//! (and every compaction that found none) for the record's search. Borrowed from Codex's own
//! switched-off mode, whose `token_budget`, `history_notes` and `new_context` stay off (run2-D9).

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, now_ms};

/// The current checkpoint.
const CURRENT: &str = "checkpoint.md";
/// Every checkpoint written, and each compaction that found none, oldest first.
const HISTORY: &str = "checkpoints.jsonl";

/// The longest checkpoint taken: it goes back whole after the compaction, beside the
/// decisions, so it stays a short record of where the work stands.
pub(crate) const CHECKPOINT_CHARS: usize = 8_000;

/// The heading Claude's summary keeps the checkpoint under ([`CLAUDE_KEEP`]).
const HEADING: &str = "## Checkpoint";

/// What a checkpoint holds (P2b), as the thread is asked for it.
const CONTENTS: &str = "the current goal; every request of the user's still open, each with \
its Brain item id and the `brigadier brain read <session> --item <id>` that shows it; the \
important command results, by item id; the files changed; the last test and its exact command; \
the next step; the skills in use";

/// The message that asks Codex's thread for its checkpoint before Brigadier compacts it.
pub(crate) fn ask() -> String {
    format!(
        "{ASK_MARK} The conversation is about to be compacted. Write your checkpoint now with \
         `brigadier checkpoint set \"<text>\"`: {CONTENTS}. Get the ids with `brigadier brain \
         search`. Never put a secret in it. Then stop; don't start new work."
    )
}

/// How Brigadier's checkpoint request starts: the thread shows it as no message of the user's.
pub(crate) const ASK_MARK: &str = "[Brigadier · checkpoint]";

/// What Claude's `/compact` adds when the checkpoint is on: its summary carries it, and
/// Brigadier keeps that section as the record (Claude can't be asked before it compacts).
pub(crate) fn claude_keep() -> String {
    format!(" End the summary with a section headed exactly \"{HEADING}\" holding: {CONTENTS}.")
}

/// Where a checkpoint came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Source {
    /// The thread wrote it (`brigadier checkpoint set`).
    Thread,
    /// Claude's summary carried it.
    Summary,
    /// A compaction found none written: the one before, if any, is stale.
    Missing,
}

/// One line of `checkpoints.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Version {
    pub at_ms: i64,
    pub source: Source,
    #[serde(default)]
    pub text: String,
}

fn current(record: &Path) -> PathBuf {
    record.join(CURRENT)
}

/// Replaces the current checkpoint with `text`, kept in the history too. Refuses an empty or
/// oversize one.
pub(crate) fn set(record: &Path, text: &str, source: Source) -> Result<()> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Error::Invalid("a checkpoint needs its text".into()));
    }
    let len = text.chars().count();
    if len > CHECKPOINT_CHARS {
        return Err(Error::Invalid(format!(
            "the checkpoint is {len} characters; keep it under {CHECKPOINT_CHARS}, with pointers \
             instead of quotes"
        )));
    }
    std::fs::create_dir_all(record).map_err(failed)?;
    crate::manager::session_dir::write_atomic(&current(record), format!("{text}\n").as_bytes())?;
    append(
        record,
        &Version {
            at_ms: now_ms(),
            source,
            text: text.to_owned(),
        },
    )
}

/// Notes that a compaction found no new checkpoint.
pub(crate) fn mark_missing(record: &Path) -> Result<()> {
    append(
        record,
        &Version {
            at_ms: now_ms(),
            source: Source::Missing,
            text: String::new(),
        },
    )
}

fn append(record: &Path, version: &Version) -> Result<()> {
    let line = serde_json::to_string(version).map_err(|err| Error::Invalid(err.to_string()))?;
    std::fs::create_dir_all(record).map_err(failed)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record.join(HISTORY))
        .and_then(|mut file| writeln!(file, "{line}"))
        .map_err(failed)
}

fn failed(err: std::io::Error) -> Error {
    Error::Invalid(format!("couldn't write the checkpoint: {err}"))
}

/// Every version, oldest first.
pub(crate) fn history(record: &Path) -> Vec<Version> {
    std::fs::read_to_string(record.join(HISTORY))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// The current checkpoint, if one was written.
pub(crate) fn read(record: &Path) -> Option<String> {
    let text = std::fs::read_to_string(current(record)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// The current checkpoint if it was written after `since_ms` (fresh for this compaction).
pub(crate) fn written_since(record: &Path, since_ms: i64) -> Option<String> {
    let last = history(record).pop()?;
    (last.source != Source::Missing && last.at_ms >= since_ms).then_some(last.text)
}

/// The checkpoint section of Claude's summary `summary`, without its heading.
pub(crate) fn summary_section(summary: &str) -> Option<String> {
    let start = summary.find(HEADING)? + HEADING.len();
    let rest = &summary[start..];
    // Up to the next heading of the same level or above.
    let end = rest
        .match_indices("\n#")
        .find(|(at, _)| !rest[at + 1..].starts_with("###"))
        .map_or(rest.len(), |(at, _)| at);
    let section = rest[..end].trim();
    (!section.is_empty()).then(|| section.to_owned())
}

/// How the checkpoint goes back after a compaction, beside the decisions.
pub(crate) fn restored(text: &str) -> String {
    format!(
        "The session's checkpoint, written just before the compaction (its working state, with \
         pointers to read again):\n{text}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "checkpoint-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn each_checkpoint_replaces_the_last_and_the_history_keeps_them_all() {
        let record = record();
        assert_eq!(read(&record), None);
        let before = now_ms();
        set(&record, "Goal: one.", Source::Thread).unwrap();
        mark_missing(&record).unwrap();
        assert_eq!(written_since(&record, before), None, "missing since");
        set(&record, "Goal: two.\nNext: test.", Source::Summary).unwrap();
        assert_eq!(read(&record).as_deref(), Some("Goal: two.\nNext: test."));
        assert_eq!(
            written_since(&record, before).as_deref(),
            Some("Goal: two.\nNext: test.")
        );
        assert_eq!(written_since(&record, now_ms() + 1_000), None);
        let sources: Vec<Source> = history(&record).iter().map(|v| v.source).collect();
        assert_eq!(sources, [Source::Thread, Source::Missing, Source::Summary]);
        assert!(set(&record, "  ", Source::Thread).is_err());
        assert!(set(&record, &"x".repeat(CHECKPOINT_CHARS + 1), Source::Thread).is_err());
        std::fs::remove_dir_all(&record).unwrap();
    }

    #[test]
    fn claudes_summary_carries_the_checkpoint_under_its_heading() {
        let summary =
            "## Summary\nWork.\n## Checkpoint\nGoal: ship.\n### Pointers\n- i-1\n## Other\nx";
        assert_eq!(
            summary_section(summary).as_deref(),
            Some("Goal: ship.\n### Pointers\n- i-1")
        );
        assert_eq!(summary_section("## Summary\nno checkpoint"), None);
        assert!(claude_keep().contains("\"## Checkpoint\""));
        assert!(ask().starts_with(ASK_MARK));
    }
}
