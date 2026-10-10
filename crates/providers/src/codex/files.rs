//! The files Codex keeps for a thread, found for Free up space (read only): its rollout in
//! `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-*-<id>.jsonl` (or `archived_sessions/`), whose
//! first line is the thread's `session_meta` (its working folder and the app that started
//! it), and the images it generated in `generated_images/<id>`. Extra accounts link all three
//! back to the main home, so they are read there once.

use std::fs::File;
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::leftovers::{Fingerprint, FoundEntry, FoundSession, is_entry, last_change};
use crate::model::ProviderKind;

/// The originator Brigadier's app-server client records on its threads.
const ORIGINATOR: &str = "brigadier";
/// The first line carries the thread's instructions, which can be long.
const LINE_LIMIT: u64 = 4 * 1024 * 1024;

pub(crate) fn leftovers(history: &Path, wanted: &dyn Fn(&Path) -> bool) -> Vec<FoundSession> {
    let mut found = Vec::new();
    for part in ["sessions", "archived_sessions"] {
        let root = history.join(part);
        let mut rollouts = Vec::new();
        collect(&root, 0, &mut rollouts);
        rollouts.sort();
        for rollout in rollouts {
            let Some((id, cwd, originator)) = meta(&rollout) else {
                continue;
            };
            if !wanted(&cwd) {
                continue;
            }
            let mut entries = vec![FoundEntry {
                root: root.clone(),
                path: rollout.clone(),
                empty_only: false,
            }];
            let images = history.join("generated_images");
            if id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                && is_entry(&images.join(&id))
            {
                entries.push(FoundEntry {
                    root: images.clone(),
                    path: images.join(&id),
                    empty_only: false,
                });
            }
            let last_change = entries
                .iter()
                .filter_map(|entry| last_change(&entry.path))
                .max();
            found.push(FoundSession {
                kind: ProviderKind::Codex,
                id,
                cwd: Some(cwd),
                fingerprint: (originator.as_deref() == Some(ORIGINATOR))
                    .then_some(Fingerprint::Originator),
                entries,
                last_change,
            });
        }
    }
    found
}

/// The rollout files under `dir`: `YYYY/MM/DD/rollout-*.jsonl`, links not followed.
fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth < 3 {
            collect(&path, depth + 1, out);
        } else if kind.is_file()
            && path.extension().is_some_and(|ext| ext == "jsonl")
            && entry.file_name().to_string_lossy().starts_with("rollout-")
        {
            out.push(path);
        }
    }
}

/// The thread's id, working folder and originator, from its `session_meta` line.
fn meta(rollout: &Path) -> Option<(String, PathBuf, Option<String>)> {
    let mut reader = BufReader::new(File::open(rollout).ok()?);
    let mut line = String::new();
    reader.by_ref().take(LINE_LIMIT).read_line(&mut line).ok()?;
    let value: Value = serde_json::from_str(&line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = value.get("payload")?;
    let id = payload.get("id").and_then(Value::as_str)?.to_owned();
    let cwd = PathBuf::from(payload.get("cwd").and_then(Value::as_str)?);
    let originator = payload
        .get("originator")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some((id, cwd, originator))
}
