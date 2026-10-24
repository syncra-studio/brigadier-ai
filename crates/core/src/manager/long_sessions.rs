//! Long sessions on the CLIs' own compaction, behind `BRIGADIER_LONG_SESSIONS` (run2-D2, run2-D15:
//! built behind a switch, off by default, on for everyone only after the test round holds up).
//!
//! Off (unset, empty or a value it doesn't know), a session goes on at seams exactly as before
//! (S22). On, a session's thread keeps one CLI session for its whole life and the CLI compacts it
//! in place, with its own compaction (Claude's `/compact` and auto-compaction, Codex's native
//! compaction); Brigadier re-implements no compaction, summary or tool-output handling. What it
//! adds, each switchable so the continuity battery can tell their effects apart
//! (`tools/continuity-battery/arms.py`, its candidate arms):
//!
//! - **restore**: after every compaction the ledger's active entries (open items, rules and
//!   decisions) go back whole and word for word, with one line on how to look up the rest
//!   (run2-D8, S22b "never cut").
//! - **instructions**: Claude's `/compact` at the compaction point carries what its summary must
//!   keep (run2-D7, D71). Codex's summary takes none.
//! - **checkpoint**: a replaceable checkpoint record with exact pointers at compactions Brigadier
//!   triggers (run2-D9, borrowed from Codex's switched-off mode, which stays off).
//!
//! Size seams go; the fresh-session seam stays for moves to another CLI (D46), quota fallback to
//! another provider (D23, D25) and as the rare fallback when compaction fails (run2-D10).

pub(crate) mod checkpoint;
pub(crate) mod restore;

use std::path::Path;

use brigadier_providers::{NativeCompaction, ProviderKind, RestoreHooks};
use serde::Deserialize;
use serde_json::Value;

use super::SessionManager;
use super::cli::CliCaller;
use super::conversation::ConvLive;
use crate::model::ConversationKind;
use crate::{Error, Result};

/// The switch, read once when the daemon starts; its value is the mode.
pub(crate) const SWITCH_ENV: &str = "BRIGADIER_LONG_SESSIONS";

/// What a session does at long contexts with the switch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LongSessions {
    /// The decisions go back whole after each compaction (run2-D8).
    pub restore: bool,
    /// Claude's `/compact` says what its summary must keep (run2-D7).
    pub instructions: bool,
    /// The checkpoint record at compactions Brigadier triggers (run2-D9).
    pub checkpoint: bool,
}

impl LongSessions {
    /// The mode named `value`, one of the battery's candidate arms. `native` and `restore-only`
    /// are the same: the first is the Codex arm's name (P1), the second the Claude ablation's (P2).
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let (restore, instructions, checkpoint) = match value.trim() {
            "native-bare" => (false, false, false),
            "native" | "restore-only" => (true, false, false),
            "instructions-only" => (false, true, false),
            "native-full" => (true, true, false),
            "native+checkpoint" => (true, false, true),
            "native-full+checkpoint" => (true, true, true),
            _ => return None,
        };
        Some(Self {
            restore,
            instructions,
            checkpoint,
        })
    }

    /// The switch as the daemon's environment sets it; `None`: off.
    pub(crate) fn from_env() -> Option<Self> {
        let value = std::env::var(SWITCH_ENV).ok()?;
        let mode = Self::parse(&value);
        if mode.is_none() && !value.trim().is_empty() {
            tracing::warn!(value = %value, "{SWITCH_ENV} names no mode; long sessions stay at seams");
        }
        mode
    }
}

impl SessionManager {
    /// The long-session mode a conversation's CLI on `provider` runs in: a session's thread with
    /// the switch on. `None`: seams as before.
    pub(super) fn long_mode(
        &self,
        conv: &ConvLive,
        provider: ProviderKind,
    ) -> Option<LongSessions> {
        let mode = self.seam_config().long?;
        (conv.kind == ConversationKind::Session
            && matches!(provider, ProviderKind::Claude | ProviderKind::Codex))
        .then_some(mode)
    }

    /// How the CLI of `conv` on `provider` compacts in place, for its session spec.
    pub(super) fn native_compaction(
        &self,
        conv: &ConvLive,
        provider: ProviderKind,
    ) -> Option<NativeCompaction> {
        let mode = self.long_mode(conv, provider)?;
        let config = self.seam_config();
        Some(match provider {
            ProviderKind::Claude => NativeCompaction {
                backstop_window: Some(backstop_window(config.tokens)),
                // The decisions go back after each compaction through hooks (run2-D8).
                restore: mode.restore.then(|| RestoreHooks {
                    command: self.config.daemon_exe.clone(),
                    args: vec![
                        "hook".into(),
                        "restore".into(),
                        "--record".into(),
                        self.session_dir(&conv.id).record().display().to_string(),
                    ],
                    slots: restore::SLOTS,
                    timeout_secs: super::tool_output::HOOK_TIMEOUT_SECS,
                }),
                ..NativeCompaction::default()
            },
            _ => NativeCompaction {
                codex_limit: config.tokens_set.then_some(config.tokens),
                ..NativeCompaction::default()
            },
        })
    }
}

/// The ledger's active entries in session record `record`, each whole and in order: its open
/// items, then its rules and decisions (what a hand-off carries verbatim, S22b).
pub(crate) fn active_entries(record: &Path) -> Vec<String> {
    let Ok(ledger) = super::cli_ledger::Ledger::read(record) else {
        return Vec::new();
    };
    ledger
        .open()
        .into_iter()
        .chain(ledger.rules())
        .map(|entry| entry.text.trim_end().to_owned())
        .collect()
}

/// Part `slot` (from 1) of the decisions put back after a compaction, read from session record
/// `record` as it stands (`brigadierd hook restore`, run2-D8). `None`: nothing for this slot,
/// or the decisions can't go back whole, which the receipt then finds (the fallback, run2-D10).
pub fn restore_part(record: &Path, slot: usize) -> Option<String> {
    let back = restore::restoration(&active_entries(record), &restore::lookup_line());
    if !back.whole {
        return None;
    }
    back.parts.into_iter().nth(slot.checked_sub(1)?)
}

/// Claude's auto-compact window as a backstop for a turn that runs on past Brigadier's point
/// `point` (run2-D5): Claude compacts by itself ≈20k below its window, so this one has it
/// compact ≈30k past the point. Claude takes windows of 100k to 1M.
fn backstop_window(point: i64) -> i64 {
    (point + 50_000).clamp(100_000, 1_000_000)
}

/// What Claude's summary must keep when Brigadier asks it to compact (run2-D7, D71): the
/// words after `/compact`. Exact pointers back to the evidence, so each fact can be read again
/// word for word, and never a secret.
pub(crate) const COMPACT_INSTRUCTIONS: &str = "\
Besides your usual summary, keep these exactly, each under its own heading: \
the open items and what is still to do; \
every decision, rule and preference the user gave, in their words, with its ledger id (L<n>) when it has one; \
every command that failed, with the command line and its error lines word for word; \
claims you checked and how (what you ran or read, and what it showed); \
the files you changed; \
the last test you ran: its exact command and result; \
the next step; \
the skills in use. \
For each fact, keep the exact pointer to where its evidence lives: file paths with line numbers, \
commit hashes, command lines, and Brain ids as `brigadier brain search` and `brigadier brain read` show them, \
so it can be read again word for word. \
Never copy a secret (a key, token or password) into the summary.";

impl LongSessions {
    /// The words after Claude's `/compact` in this mode: what its summary must keep (run2-D7)
    /// and, with the checkpoint, the section that carries it (run2-D9). `None` without either
    /// (as in `native-bare`).
    pub(crate) fn compact_instructions(self) -> Option<String> {
        let keep = [
            self.instructions.then(|| COMPACT_INSTRUCTIONS.to_owned()),
            self.checkpoint.then(checkpoint::claude_keep),
        ];
        let words: String = keep.into_iter().flatten().collect();
        let words = words.trim();
        (!words.is_empty()).then(|| words.to_owned())
    }
}

impl SessionManager {
    /// `brigadier checkpoint set|show`: the session thread's checkpoint record (run2-D9). It
    /// works with the switch off too: the record is only read back when the mode is on.
    pub(super) async fn cli_checkpoint(
        &self,
        caller: &CliCaller,
        action: &str,
        given: Value,
    ) -> Result<String> {
        // Check the stored conversation too: a tab chat must never leave a kept record.
        let conv = self.core.conversation(&caller.conversation_id)?;
        if conv.tab_chat.is_some()
            || conv.kind != ConversationKind::Session
            || caller.task_id.is_some()
        {
            return Err(Error::Invalid(
                "Only a session's thread keeps a checkpoint.".into(),
            ));
        }
        let record = self.session_dir(&caller.conversation_id).record();
        let invalid =
            |err: serde_json::Error| Error::Invalid(format!("bad checkpoint call: {err}"));
        match action {
            "set" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Set {
                    text: String,
                }
                let set: Set = serde_json::from_value(given).map_err(invalid)?;
                super::blocking(move || {
                    checkpoint::set(&record, &set.text, checkpoint::Source::Thread)?;
                    Ok("Checkpoint saved.\n".to_owned())
                })
                .await
            }
            "show" => {
                super::blocking(move || {
                    Ok(checkpoint::read(&record).map_or_else(
                        || "No checkpoint yet.\n".to_owned(),
                        |text| format!("{text}\n"),
                    ))
                })
                .await
            }
            other => Err(Error::Invalid(format!("no checkpoint action {other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_candidate_arm_of_the_battery_is_a_mode() {
        let on = |restore, instructions, checkpoint| {
            Some(LongSessions {
                restore,
                instructions,
                checkpoint,
            })
        };
        for (arm, mode) in [
            ("native-bare", on(false, false, false)),
            ("native", on(true, false, false)),
            ("restore-only", on(true, false, false)),
            ("instructions-only", on(false, true, false)),
            ("native-full", on(true, true, false)),
            ("native+checkpoint", on(true, false, true)),
            ("native-full+checkpoint", on(true, true, true)),
        ] {
            assert_eq!(LongSessions::parse(arm), mode, "{arm}");
        }
        for off in ["", "1", "on", "seam", "plain", "native-ful"] {
            assert_eq!(LongSessions::parse(off), None, "{off:?}");
        }
    }
}
