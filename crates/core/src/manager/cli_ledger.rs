//! The caller's kept ledger, writable through `brigadier` even in a Chat.

use std::io::Write;
use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

use super::SessionManager;
use super::cli::CliCaller;
use super::session_dir::LEDGER_FILE;
use crate::{Error, Result};

impl SessionManager {
    pub(super) async fn cli_ledger(
        &self,
        caller: &CliCaller,
        action: &str,
        given: Value,
    ) -> Result<String> {
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
        let path = record.join(LEDGER_FILE);
        match action {
            "add" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Entry {
                    text: String,
                }
                let entry: Entry = serde_json::from_value(given)
                    .map_err(|err| Error::Invalid(format!("bad ledger entry: {err}")))?;
                super::blocking(move || append(&path, &entry.text)).await?;
                Ok(String::new())
            }
            "show" => super::blocking(move || read(&path)).await,
            other => Err(Error::Invalid(format!("no ledger action {other:?}"))),
        }
    }
}

fn read(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(Error::Invalid(format!("couldn't read the ledger: {err}"))),
    }
}

fn append(path: &Path, text: &str) -> Result<()> {
    // One append per call, including when a session runs commands concurrently. Never
    // rewrite the old entries: threads can also append with their own file tools.
    static APPENDS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = APPENDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let text = text.trim();
    let text = ["- ", "* ", "+ "]
        .iter()
        .find_map(|marker| text.strip_prefix(marker))
        .unwrap_or(text);
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
    let old = read(path)?;
    let marker = old
        .lines()
        .rev()
        .find_map(|line| {
            ["- ", "* ", "+ "]
                .into_iter()
                .find(|marker| line.starts_with(marker))
        })
        .unwrap_or("- ");
    let newline = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let separator = if old.is_empty() || old.ends_with('\n') {
        ""
    } else {
        newline
    };
    let entry = format!("{separator}{marker}{text}{newline}");
    let failed = |err| Error::Invalid(format!("couldn't append to the ledger: {err}"));
    std::fs::create_dir_all(path.parent().expect("a ledger is in record/")).map_err(failed)?;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(entry.as_bytes()))
        .map_err(failed)
}
