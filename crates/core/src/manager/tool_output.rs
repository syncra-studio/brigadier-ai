//! Lossless trimming of the thread's command output (THREAD-PLAN.md Q4).
//!
//! A long output is stored whole in the blob store first, under a short alias (`out-<id>`)
//! that the conversation owns ([`StoredOutput`], recorded on its stream: the event's mention of
//! the blob keeps it stored while the conversation lives, and deleting the conversation takes
//! it). The model gets a digest ([`crate::digest`]) naming the stored file, which it reads with
//! its own tools.
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
use crate::digest::{BASH_WHOLE_MAX, digest, wrapped_digest};
use crate::model::{ConversationId, DomainEvent};
use crate::tools::Role;
use crate::work::{OutputSource, StoredOutput};
use crate::{Result, now_ms};

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
        self.store_output_as(id, source, status, output, trimmed, wrapped, false)
            .await
    }

    /// `output` of a command the thread of `id` ran, without the session's grants (an `env`)
    /// or the project's secrets (a `cat .env`): neither reaches the model or the disk.
    pub(crate) async fn hide_secrets(&self, id: &ConversationId, output: Vec<u8>) -> Vec<u8> {
        let mut hidden = self.grants.secrets();
        hidden.extend(self.session_secret_values(id).await);
        // Output that isn't UTF-8 is read the way the thread is shown it, so a stray byte
        // doesn't let a secret through.
        match super::secrets::redactor(hidden) {
            Some(redactor) => redactor
                .redact(&String::from_utf8_lossy(&output))
                .into_owned()
                .into_bytes(),
            None => output,
        }
    }

    /// [`Self::store_output`], its digest sized for a model that reads it JSON-escaped when
    /// `wrapped`, or a passing check's reply ([`crate::digest::passed`]) when `passed`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn store_output_as(
        &self,
        id: &ConversationId,
        source: OutputSource,
        status: &str,
        output: Vec<u8>,
        trimmed: bool,
        wrapped: bool,
        passed: bool,
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
        let output = self.hide_secrets(id, output).await;
        // Stored first: the digest names the file the model reads the rest from.
        let blobs = self.core.store().blobs().clone();
        let (blob, output) = super::blocking(move || {
            let blob = blobs.put_blocking(&output)?;
            Ok((blob, output))
        })
        .await?;
        let artifact = format!(
            "{alias} in {}",
            self.core.store().blobs().file_of(&blob).display()
        );
        let digest = if passed {
            crate::digest::passed(status, &output, &artifact)
        } else if wrapped {
            wrapped_digest(status, &output, &artifact)
        } else {
            digest(status, &output, &artifact)
        };
        let lines = output.split(|byte| *byte == b'\n').count() as u64
            - u64::from(output.ends_with(b"\n") || output.is_empty());
        let bytes = output.len() as u64;
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
    /// the model: the digest of a whole output over [`BASH_WHOLE_MAX`] bytes; nothing for a
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
            HookOutput::Full if output.len() <= BASH_WHOLE_MAX => Ok(None),
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
}
