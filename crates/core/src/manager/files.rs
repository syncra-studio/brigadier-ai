//! The side panel's Files tab: one file of a session's checkout, read for the
//! user to look at. It only reads, and never outside the checkout.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::{SessionManager, blocking};
use crate::model::{ConversationId, Environment, Setup};
use crate::work::CheckoutFile;
use crate::{Error, Result};

/// The most of a file the tab shows; a longer one is cut here and says so.
const MAX_TEXT: u64 = 1024 * 1024;
/// How far into a file a NUL byte marks it as binary (as git decides).
const BINARY_PROBE: usize = 8000;

impl SessionManager {
    /// Where the user looks at a session's files and runs its terminal: its checkout, or the
    /// repository while its worktree doesn't exist yet.
    pub fn checkout_dir(&self, id: &ConversationId) -> Result<String> {
        let Some(Setup::Session {
            repo, environment, ..
        }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid("only a session has a checkout".into()));
        };
        Ok(match environment {
            Environment::LocalCheckout { .. } => repo,
            Environment::NewWorktree { path, .. } => path.unwrap_or(repo),
        })
    }

    /// The file at `path`, relative to the session's checkout, or by its full path a file of
    /// the session's own record (its ledger, S17): its text, or that it is binary.
    pub async fn read_file(&self, id: &ConversationId, path: String) -> Result<CheckoutFile> {
        let record = self.session_dir(id).record();
        if let Ok(inside) = Path::new(&path).strip_prefix(&record) {
            let inside = inside.to_string_lossy().into_owned();
            let mut file = blocking(move || read_checkout_file(&record, &inside)).await?;
            file.path = path;
            return Ok(file);
        }
        let root = self.checkout_dir(id)?;
        blocking(move || read_checkout_file(Path::new(&root), &path)).await
    }
}

fn read_checkout_file(root: &Path, path: &str) -> Result<CheckoutFile> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::Invalid(format!(
            "`{path}` is not a path in the checkout"
        )));
    }
    let unreadable = |err: std::io::Error| Error::Invalid(format!("couldn't read {path}: {err}"));
    let root = root.canonicalize().map_err(unreadable)?;
    // Resolved, so a symlink can't lead out of the checkout.
    let full: PathBuf = root.join(relative).canonicalize().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            Error::NotFound(path.to_owned())
        } else {
            unreadable(err)
        }
    })?;
    if !full.starts_with(&root) {
        return Err(Error::Invalid(format!("{path} is outside the checkout")));
    }
    let file = File::open(&full).map_err(unreadable)?;
    let metadata = file.metadata().map_err(unreadable)?;
    if !metadata.is_file() {
        return Err(Error::Invalid(format!("{path} is not a file")));
    }
    let size = metadata.len();
    let mut bytes = Vec::new();
    file.take(MAX_TEXT)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    let truncated = size > MAX_TEXT;
    let binary = bytes[..bytes.len().min(BINARY_PROBE)].contains(&0);
    let text = (!binary).then(|| match String::from_utf8(bytes) {
        Ok(text) => text,
        // A cut can split a character, and a file may not be UTF-8 at all.
        Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
    });
    Ok(CheckoutFile {
        path: path.to_owned(),
        size,
        text,
        truncated,
    })
}
