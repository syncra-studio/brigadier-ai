//! A session's checkpoint (P2b, run2-D9): its working state, with exact pointers back to the
//! source, so it can be read again word for word. Pull-only (D13): the thread writes it with
//! `brigadier checkpoint set` and reads it back with `show` or `brigadier context`; nothing
//! asks for it or puts it back into the conversation. A
//! separate, replaceable record, never ledger entries (D28: the ledger stays append-only):
//! `record/checkpoint.md` is the current one, `record/checkpoints.jsonl` keeps every version
//! for the record's search.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, now_ms};

/// The current checkpoint.
const CURRENT: &str = "checkpoint.md";
/// Every checkpoint written, and each compaction that found none, oldest first.
const HISTORY: &str = "checkpoints.jsonl";

/// The longest checkpoint taken, so it stays a short record of where the work stands.
pub(crate) const CHECKPOINT_CHARS: usize = 8_000;

/// Where a checkpoint came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Source {
    /// The thread wrote it (`brigadier checkpoint set`).
    Thread,
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
#[cfg(test)]
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
        set(&record, "Goal: one.", Source::Thread).unwrap();
        set(&record, "Goal: two.\nNext: test.", Source::Thread).unwrap();
        assert_eq!(read(&record).as_deref(), Some("Goal: two.\nNext: test."));
        let texts: Vec<String> = history(&record).into_iter().map(|v| v.text).collect();
        assert_eq!(texts, ["Goal: one.", "Goal: two.\nNext: test."]);
        assert!(set(&record, "  ", Source::Thread).is_err());
        assert!(set(&record, &"x".repeat(CHECKPOINT_CHARS + 1), Source::Thread).is_err());
        std::fs::remove_dir_all(&record).unwrap();
    }
}
