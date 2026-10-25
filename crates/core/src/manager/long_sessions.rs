//! Long sessions on the CLIs' own compaction (run2-D2), the default for every session's thread
//! (D15, superseding run2-D15's switch, which kept it off).
//!
//! A session's thread keeps one CLI session for its whole life and the CLI compacts it in
//! place, with its own compaction (Claude's `/compact` and auto-compaction, Codex's native
//! compaction); Brigadier re-implements no compaction, summary or tool-output handling. It puts
//! nothing into the conversation after a compaction (D13, superseding run2-D8's restore):
//! everything about the job is pulled with `brigadier context`, which the standing instructions
//! and Claude's steered `/compact` point to. What it adds:
//!
//! - **instructions**: Claude's `/compact` at the compaction point carries what its summary must
//!   keep, ending on the pointer to `brigadier context` (run2-D7, D71, D13). Codex's summary
//!   takes none.
//! - **hand-off after 5 compactions** in one CLI session (D16), and as the rare fallback when
//!   compaction fails or loops (run2-D10): the fresh-session seam.
//!
//! The fresh-session seam also stays for moves to another CLI (D46) and quota fallback to
//! another provider (D23, D25). `BRIGADIER_LONG_SESSIONS` is a test-only opt-out, never a user
//! setting: `seams` brings back the size seams, `native-bare` drops the keep-instructions (the
//! continuity battery's arms, `tools/continuity-battery/arms.py`).

pub(crate) mod checkpoint;

use brigadier_providers::{NativeCompaction, ProviderKind};
use serde::Deserialize;
use serde_json::Value;

use super::SessionManager;
use super::cli::CliCaller;
use super::conversation::ConvLive;
use crate::model::ConversationKind;
use crate::{Error, Result};

/// The test-only opt-out, read once when the daemon starts.
pub(crate) const SWITCH_ENV: &str = "BRIGADIER_LONG_SESSIONS";

/// How many compactions in a row one CLI session of a session's thread takes before a fresh
/// one goes on from its hand-off note (D16: each compaction summarises the one before).
pub(crate) const SESSION_HANDOFF_AFTER: u32 = 5;

/// What a session does at long contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LongSessions {
    /// Claude's `/compact` says what its summary must keep (run2-D7).
    pub instructions: bool,
}

impl Default for LongSessions {
    fn default() -> Self {
        Self { instructions: true }
    }
}

impl LongSessions {
    /// The mode `value` names: `None` for `seams` (size seams, as before D15). Unset, empty or
    /// unknown: the default.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            // `seam-repaired` was the battery's size seams with P3's hand-off text.
            "seams" | "seam-repaired" => None,
            "native-bare" => Some(Self {
                instructions: false,
            }),
            // The battery's arms that put the decisions back or asked for a checkpoint run as
            // the default since D13: nothing is put back.
            ""
            | "default"
            | "instructions-only"
            | "native"
            | "restore-only"
            | "native-full"
            | "native+checkpoint"
            | "native-full+checkpoint" => Some(Self::default()),
            other => {
                tracing::warn!(value = %other, "{SWITCH_ENV} names no mode; long sessions compact in place");
                Some(Self::default())
            }
        }
    }

    /// The mode the daemon's environment asks for; the default without one.
    pub(crate) fn from_env() -> Option<Self> {
        Self::parse(&std::env::var(SWITCH_ENV).unwrap_or_default())
    }
}

impl SessionManager {
    /// The long-session mode a conversation's CLI on `provider` runs in: a session's thread,
    /// unless the test-only opt-out asks for seams. `None`: seams as before.
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
        self.long_mode(conv, provider)?;
        let config = self.seam_config();
        Some(match provider {
            ProviderKind::Claude => NativeCompaction {
                backstop_window: Some(backstop_window(config.tokens)),
                ..NativeCompaction::default()
            },
            _ => NativeCompaction {
                codex_limit: config.tokens_set.then_some(config.tokens),
                ..NativeCompaction::default()
            },
        })
    }
}

/// Claude's auto-compact window as a backstop for a turn that runs on past Brigadier's point
/// `point` (run2-D5): Claude compacts by itself ≈20k below its window, so this one has it
/// compact ≈30k past the point. Claude takes windows of 100k to 1M.
fn backstop_window(point: i64) -> i64 {
    (point + 50_000).clamp(100_000, 1_000_000)
}

/// What Claude's summary must keep when Brigadier asks it to compact (run2-D7, D71): the
/// words after `/compact`. Exact pointers back to the evidence, so each fact can be read again
/// word for word, and never a secret. Its last line points to `brigadier context` (D13), where
/// the decisions, open items and ledger are, instead of Brigadier putting them back.
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
Never copy a secret (a key, token or password) into the summary. \
End the summary with this line: next: run `brigadier context`.";

impl LongSessions {
    /// The words after Claude's `/compact` in this mode: what its summary must keep (run2-D7).
    /// `None` in `native-bare`.
    pub(crate) fn compact_instructions(self) -> Option<String> {
        self.instructions.then(|| COMPACT_INSTRUCTIONS.to_owned())
    }
}

impl SessionManager {
    /// `brigadier checkpoint set|show`: the session thread's checkpoint record (run2-D9), pulled
    /// only: nothing puts it back into the conversation (D13).
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
    fn compaction_in_place_is_the_default_and_seams_only_a_test_opt_out() {
        let on = |instructions| Some(LongSessions { instructions });
        for (value, mode) in [
            ("", on(true)),
            ("default", on(true)),
            ("instructions-only", on(true)),
            ("unknown", on(true)),
            ("native-bare", on(false)),
            ("native-full+checkpoint", on(true)),
            ("seams", None),
            ("seam-repaired", None),
        ] {
            assert_eq!(LongSessions::parse(value), mode, "{value:?}");
        }
    }

    #[test]
    fn claudes_compact_summary_ends_on_the_context_pointer() {
        let words = LongSessions::default().compact_instructions().unwrap();
        assert!(words.ends_with("next: run `brigadier context`."), "{words}");
        assert_eq!(
            LongSessions {
                instructions: false
            }
            .compact_instructions(),
            None
        );
    }
}
