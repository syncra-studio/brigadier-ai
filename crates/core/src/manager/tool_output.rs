//! Lossless trimming of the thread's command output (THREAD-PLAN.md Q4).
//!
//! A long output is stored whole in the blob store first, under a short alias (`out-<id>`)
//! that the conversation owns ([`StoredOutput`], recorded on its stream: the event's mention of
//! the blob keeps it stored while the conversation lives, and deleting the conversation takes
//! it). The model gets a digest ([`crate::digest`]) and pages through the rest with
//! `read_artifact`, which takes an alias only from the conversation that owns it.
//!
//! - A Claude thread's `Bash` output reaches Brigadier through its `PostToolUse` hook
//!   (`brigadierd hook post-tool-use`), with a grant that can only do this
//!   ([`Role::OutputHook`]). A failing call's hook gets only the CLI's excerpt of the output; it
//!   is stored as such, and the model gets it untrimmed
//!   (docs/evidence/2026-10-07-thread-phase2-contracts.md §3).
//! - A Codex thread's `run` tool owns its command, so every long result is trimmed, failures
//!   included (`super::run`). Its model reads the digest JSON-escaped inside its code-mode
//!   tool's output, so that digest is sized for it ([`crate::digest::wrapped_digest`]).

use super::SessionManager;
use crate::digest::{TRIM_ABOVE, digest, wrapped_digest};
use crate::model::{ConversationId, DomainEvent};
use crate::tools::Role;
use crate::work::{OutputSource, StoredOutput};
use crate::{Error, Result, now_ms};

/// Largest output stored; what a command prints beyond it is counted, not kept.
pub const OUTPUT_MAX_BYTES: usize = 64 * 1024 * 1024;
/// The environment variable that holds a hook's grant (set in the CLI's environment, which its
/// hooks inherit).
pub const HOOK_GRANT_ENV: &str = "BRIGADIER_HOOK_GRANT";
/// How long Claude lets the hook take (it reads at most one output file and stores it).
pub(crate) const HOOK_TIMEOUT_SECS: u64 = 60;
/// The prefix of an output's alias.
const ALIAS_PREFIX: &str = "out-";

/// What a hook stores ([`SessionManager::hook_output`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutput {
    /// A successful call's whole output: the reply is its digest, when it is long enough to
    /// trim.
    Full,
    /// A failing call's excerpt, the most the CLI gives a failure's hook: stored as it is, and
    /// nothing replaces it.
    Excerpt,
}

impl SessionManager {
    /// Stores `output`, a command's output of the thread of `id` that ended with `status`,
    /// under a new alias, and records it on the conversation. Returns the record and the
    /// digest of it.
    pub(crate) async fn store_output(
        &self,
        id: &ConversationId,
        source: OutputSource,
        status: &str,
        output: Vec<u8>,
        trimmed: bool,
    ) -> Result<(StoredOutput, String)> {
        // A Codex thread reads `run`'s result JSON-escaped inside its code-mode tool's output.
        let wrapped = source == OutputSource::Run;
        self.store_output_as(id, source, status, output, trimmed, wrapped)
            .await
    }

    /// [`Self::store_output`], its digest sized for a model that reads it JSON-escaped when
    /// `wrapped`.
    pub(crate) async fn store_output_as(
        &self,
        id: &ConversationId,
        source: OutputSource,
        status: &str,
        output: Vec<u8>,
        trimmed: bool,
        wrapped: bool,
    ) -> Result<(StoredOutput, String)> {
        let board = self.core.board(id).await?;
        let alias = loop {
            let alias = format!(
                "{ALIAS_PREFIX}{}",
                &uuid::Uuid::new_v4().simple().to_string()[..8]
            );
            if !board.outputs.contains_key(&alias) {
                break alias;
            }
        };
        // A command that printed the session's grants (`env`) doesn't keep them on disk.
        let output = match (
            super::secrets::redactor(self.grants.secrets()),
            std::str::from_utf8(&output),
        ) {
            (Some(redactor), Ok(text)) => redactor.redact(text).into_owned().into_bytes(),
            _ => output,
        };
        let digest = if wrapped {
            wrapped_digest(status, &output, &alias)
        } else {
            digest(status, &output, &alias)
        };
        let lines = output.split(|byte| *byte == b'\n').count() as u64
            - u64::from(output.ends_with(b"\n") || output.is_empty());
        let bytes = output.len() as u64;
        let blob = self.core.store().blobs().put(output).await?;
        let stored = StoredOutput {
            alias,
            blob: blob.to_string(),
            source,
            status: status.to_owned(),
            bytes,
            lines,
            shown_bytes: trimmed.then_some(digest.len() as u64),
            at_ms: now_ms(),
        };
        self.core
            .record_conversation(
                id,
                vec![DomainEvent::OutputStored {
                    conversation_id: id.clone(),
                    output: stored.clone(),
                }],
            )
            .await?;
        Ok((stored, digest))
    }

    /// What a thread's output hook sends (`brigadierd hook post-tool-use`): a successful
    /// call's whole output, or a failing call's excerpt. Returns what replaces the output for
    /// the model: the digest of a whole output over [`TRIM_ABOVE`] bytes; nothing for a
    /// shorter one or an excerpt. `None` from the grant: it isn't a live hook grant.
    pub async fn hook_output(
        &self,
        grant: &str,
        kind: HookOutput,
        status: &str,
        output: Vec<u8>,
    ) -> Option<Result<Option<String>>> {
        let Some(Role::OutputHook { conversation_id }) = self.grants.resolve(grant) else {
            return None;
        };
        Some(match kind {
            HookOutput::Full if output.len() <= TRIM_ABOVE => Ok(None),
            HookOutput::Full => self
                .store_output(&conversation_id, OutputSource::Bash, status, output, true)
                .await
                .map(|(_, digest)| Some(digest)),
            HookOutput::Excerpt => self
                .store_output(
                    &conversation_id,
                    OutputSource::BashExcerpt,
                    status,
                    output,
                    false,
                )
                .await
                .map(|_| None),
        })
    }

    /// The blob of the conversation's output `alias`, and what to call it; an alias of another
    /// conversation, or none, is refused.
    pub(crate) async fn output_blob(
        &self,
        id: &ConversationId,
        alias: &str,
    ) -> Result<Option<(String, String)>> {
        if !alias.starts_with(ALIAS_PREFIX) {
            return Ok(None);
        }
        let board = self.core.board(id).await?;
        let output = board.outputs.get(alias).ok_or_else(|| {
            Error::Invalid(format!(
                "{alias} is not a command output of this session. Use the out-… id a trimmed \
                 output named."
            ))
        })?;
        let name = match output.source {
            OutputSource::BashExcerpt => "the stored excerpt of a command's output",
            OutputSource::Bash | OutputSource::Run | OutputSource::Check => {
                "a command's full output"
            }
            OutputSource::Preview => "a preview's log",
        };
        Ok(Some((output.blob.clone(), name.into())))
    }
}
