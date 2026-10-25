//! The files Codex keeps for a thread, found for Free up space (read only): its rollout in
//! `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-*-<id>.jsonl` (or `archived_sessions/`), whose
//! first line is the thread's `session_meta` (its working folder and the app that started
//! it), and the images it generated in `generated_images/<id>`. Extra accounts link all three
//! back to the main home, so they are read there once.
//!
//! Besides what `thread/delete` removes, Codex keeps a thread's traces in the home it ran from
//! (an extra account's own, not linked): shell snapshots `shell_snapshots/<id>.<nanos>.sh`, its
//! prompts in `history.jsonl` (`session_id`), its name in `session_index.jsonl` (`id`) and its
//! log rows in `logs_2.sqlite` (`logs.thread_id`); [`remove_thread_extras`] removes them.

use std::fs::File;
use std::io::{BufRead, BufReader, Read as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::leftovers::{Fingerprint, FoundEntry, FoundSession, is_entry, last_change};
use crate::model::ProviderKind;
use crate::{Error, Result};

/// How long removing a thread's log rows waits for a Codex that is writing logs.
const LOGS_BUSY: Duration = Duration::from_secs(5);

/// Removes what Codex keeps of thread `thread_id` in `home` besides its rollout (see the module
/// docs). Each part is tried; the first failure is returned. Missing files are fine.
pub(crate) fn remove_thread_extras(home: &Path, thread_id: &str) -> Result<()> {
    if thread_id.is_empty()
        || !thread_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(Error::Invalid(format!(
            "{thread_id} is not a Codex thread id"
        )));
    }
    let results = [
        remove_shell_snapshots(home, thread_id),
        remove_lines(&home.join("history.jsonl"), "session_id", thread_id),
        remove_lines(&home.join("session_index.jsonl"), "id", thread_id),
        remove_log_rows(&home.join("logs_2.sqlite"), thread_id),
    ];
    results.into_iter().collect()
}

/// `shell_snapshots/<thread_id>.<anything>.sh`.
fn remove_shell_snapshots(home: &Path, thread_id: &str) -> Result<()> {
    let dir = home.join("shell_snapshots");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    let prefix = format!("{thread_id}.");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(&prefix)
            && name.ends_with(".sh")
            && name.len() > prefix.len() + ".sh".len()
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            match std::fs::remove_file(entry.path()) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                    return Err(failed(&entry.path(), err));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn remove_lines(file: &Path, key: &str, thread_id: &str) -> Result<()> {
    crate::history_lines::remove_lines(file, |line| {
        line.get(key).and_then(Value::as_str) == Some(thread_id)
    })
    .map_err(|err| failed(file, err))
}

/// Deletes the thread's rows from Codex's log database, when it has one with a `logs` table
/// that names threads.
fn remove_log_rows(db: &Path, thread_id: &str) -> Result<()> {
    use rusqlite::{Connection, OpenFlags, OptionalExtension as _};
    if !db.is_file() {
        return Ok(());
    }
    let sqlite = |err: rusqlite::Error| failed(db, std::io::Error::other(err));
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(sqlite)?;
    conn.busy_timeout(LOGS_BUSY).map_err(sqlite)?;
    let names_threads = conn
        .query_row(
            "SELECT 1 FROM pragma_table_info('logs') WHERE name = 'thread_id'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(sqlite)?
        .is_some();
    if names_threads {
        // Nor a copy in the freed pages or the write-ahead log (D8).
        conn.execute_batch("PRAGMA secure_delete = ON")
            .map_err(sqlite)?;
        conn.execute("DELETE FROM logs WHERE thread_id = ?1", [thread_id])
            .map_err(sqlite)?;
        // A Codex writing logs meanwhile keeps the log busy: its next checkpoint empties it.
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
    }
    Ok(())
}

fn failed(path: &Path, err: std::io::Error) -> Error {
    Error::Io(std::io::Error::new(
        err.kind(),
        format!("{}: {err}", path.display()),
    ))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "01a16818-48af-7f62-9ab6-cc3e9afd5c9c";
    const OTHER: &str = "01a16818-4b48-74d1-8f0d-30c8c436d8f6";

    fn line(key: &str, id: &str, text: &str) -> String {
        format!(
            "{}\n",
            serde_json::json!({ key: id, "ts": 1, "text": text })
        )
    }

    #[test]
    fn a_threads_snapshots_prompts_name_and_logs_go_and_other_threads_stay() {
        let home =
            std::env::temp_dir().join(format!("brigadier-codex-home-{}", uuid::Uuid::new_v4()));
        let snapshots = home.join("shell_snapshots");
        std::fs::create_dir_all(&snapshots).unwrap();
        for name in [
            format!("{THREAD}.1792748731166141000.sh"),
            format!("{THREAD}.1792748752410849000.sh"),
            format!("{OTHER}.1792748731166141000.sh"),
            format!("{THREAD}.1792748731166141000.sh.keep"),
            format!("{THREAD}-not-it.1.sh"),
        ] {
            std::fs::write(snapshots.join(name), "export A=1\n").unwrap();
        }
        let history = home.join("history.jsonl");
        std::fs::write(
            &history,
            line("session_id", THREAD, "hi") + &line("session_id", OTHER, "there"),
        )
        .unwrap();
        let index = home.join("session_index.jsonl");
        std::fs::write(&index, line("id", OTHER, "b") + &line("id", THREAD, "a")).unwrap();
        let db = home.join("logs_2.sqlite");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            conn.execute_batch(
                "CREATE TABLE logs (id INTEGER PRIMARY KEY AUTOINCREMENT, ts INTEGER NOT NULL, \
                 thread_id TEXT, feedback_log_body TEXT);",
            )
            .unwrap();
            for (thread, body) in [
                (Some(THREAD), "a"),
                (Some(OTHER), "b"),
                (None, "c"),
                (Some(THREAD), "d"),
            ] {
                conn.execute(
                    "INSERT INTO logs (ts, thread_id, feedback_log_body) VALUES (1, ?1, ?2)",
                    rusqlite::params![thread, body],
                )
                .unwrap();
            }
        }

        remove_thread_extras(&home, THREAD).unwrap();

        let mut left: Vec<String> = std::fs::read_dir(&snapshots)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                format!("{THREAD}-not-it.1.sh"),
                format!("{THREAD}.1792748731166141000.sh.keep"),
                format!("{OTHER}.1792748731166141000.sh"),
            ]
        );
        assert_eq!(
            std::fs::read_to_string(&history).unwrap(),
            line("session_id", OTHER, "there")
        );
        assert_eq!(
            std::fs::read_to_string(&index).unwrap(),
            line("id", OTHER, "b")
        );
        let conn = rusqlite::Connection::open(&db).unwrap();
        let bodies: Vec<String> = conn
            .prepare("SELECT feedback_log_body FROM logs ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(bodies, ["b", "c"]);
        drop(conn);

        // Again, and in a home with none of it: nothing to do, and no database made.
        remove_thread_extras(&home, THREAD).unwrap();
        let empty = home.join("empty");
        std::fs::create_dir(&empty).unwrap();
        remove_thread_extras(&empty, THREAD).unwrap();
        assert!(!empty.join("logs_2.sqlite").exists());
        // A log database whose table names no threads is left alone.
        rusqlite::Connection::open(empty.join("logs_2.sqlite"))
            .unwrap()
            .execute_batch("CREATE TABLE logs (id INTEGER PRIMARY KEY);")
            .unwrap();
        remove_thread_extras(&empty, THREAD).unwrap();

        for bad in ["../x", "", "a/b", "*"] {
            assert!(
                matches!(remove_thread_extras(&home, bad), Err(Error::Invalid(_))),
                "{bad}"
            );
        }
        std::fs::remove_dir_all(&home).unwrap();
    }
}
