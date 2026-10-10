//! The session files each CLI keeps on disk, found for Free up space: read only (never
//! changed here), with what the files themselves say about who started the session.
//!
//! One place per CLI: [`find`] asks the module that knows that CLI's files. A new CLI adds its
//! arm there; Brigadier's core decides from what is found whether a session is its leftover.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::model::ProviderKind;

/// A CLI session's files as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundSession {
    pub kind: ProviderKind,
    /// The CLI's own id for it (Claude session id, Codex thread id).
    pub id: String,
    /// The folder it ran in, as its files say.
    pub cwd: Option<PathBuf>,
    /// What in its files shows Brigadier started it; `None`: nothing does.
    pub fingerprint: Option<Fingerprint>,
    /// Its files and folders, each with the folder it must stay inside, in the order they can
    /// go (a folder that must be empty first comes after what is in it).
    pub entries: Vec<FoundEntry>,
    /// When any of its files last changed.
    pub last_change: Option<SystemTime>,
}

/// A file or folder of a found session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundEntry {
    pub root: PathBuf,
    pub path: PathBuf,
    /// Removed only once empty: a folder other sessions' files may share.
    pub empty_only: bool,
}

/// Brigadier's own marks in a session's files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fingerprint {
    /// Claude: started headless (`entrypoint` `sdk-cli`) and called Brigadier's own tools
    /// (`mcp__brigadier__…`).
    BrigadierTools,
    /// Claude: started headless and its sandbox let it reach a Brigadier daemon's socket, the
    /// one of the data directory given (`<data>/run/brigadierd.sock`).
    BrigadierSocket { data_dir: PathBuf },
    /// Codex: the thread's recorded originator is Brigadier.
    Originator,
}

impl Fingerprint {
    /// The data directory the mark names, when it names one.
    pub fn data_dir(&self) -> Option<&Path> {
        match self {
            Self::BrigadierSocket { data_dir } => Some(data_dir),
            _ => None,
        }
    }

    /// Why the files are Brigadier's, in plain words.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::BrigadierTools => "it used Brigadier's tools",
            Self::BrigadierSocket { .. } => "it was connected to Brigadier",
            Self::Originator => "Codex recorded Brigadier as the app that started it",
        }
    }
}

/// The sessions of `kind` whose working folder `wanted` accepts.
///
/// `history` is the CLI's main home, where its shared history lives (extra accounts link
/// theirs back to it); `homes` are every home with per-session state of its own (the main home
/// and each extra account's). Only the sessions `wanted` picks are read in full.
pub fn find(
    kind: ProviderKind,
    history: &Path,
    homes: &[PathBuf],
    wanted: &dyn Fn(&Path) -> bool,
) -> Vec<FoundSession> {
    match kind {
        ProviderKind::Claude => crate::claude::files::leftovers(history, homes, wanted),
        ProviderKind::Codex => crate::codex::files::leftovers(history, wanted),
    }
}

/// When `path` (a file, or a folder and everything in it) last changed, links not followed.
pub(crate) fn last_change(path: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut stack = vec![path.to_owned()];
    while let Some(next) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            continue;
        };
        if let Ok(modified) = meta.modified() {
            newest = Some(newest.map_or(modified, |newest| newest.max(modified)));
        }
        if meta.is_dir()
            && let Ok(entries) = std::fs::read_dir(&next)
        {
            stack.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    newest
}

/// Whether `path` is there and is not a link.
pub(crate) fn is_entry(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| !meta.file_type().is_symlink())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A fresh folder, removed after the test however it ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-leftovers-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir.canonicalize().unwrap())
    }

    fn lines(values: &[serde_json::Value]) -> String {
        values
            .iter()
            .map(|value| format!("{value}\n"))
            .collect::<String>()
    }

    /// A Claude transcript of `id` run in `cwd`, started as `entrypoint`, with `extra` records.
    fn transcript(
        home: &Path,
        cwd: &str,
        id: &str,
        entrypoint: &str,
        extra: &[serde_json::Value],
    ) -> PathBuf {
        let folder = home.join("projects").join(
            cwd.chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect::<String>(),
        );
        std::fs::create_dir_all(&folder).unwrap();
        let mut records = vec![
            json!({"type": "queue-operation", "sessionId": id}),
            json!({"type": "user", "entrypoint": entrypoint, "cwd": cwd, "sessionId": id,
                   "message": {"role": "user", "content": "mcp__brigadier__delegate_task please"}}),
        ];
        records.extend_from_slice(extra);
        let path = folder.join(format!("{id}.jsonl"));
        std::fs::write(&path, lines(&records)).unwrap();
        path
    }

    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";
    const C: &str = "33333333-3333-4333-8333-333333333333";
    const D: &str = "44444444-4444-4444-8444-444444444444";

    #[test]
    fn claude_fingerprint_comes_only_from_brigadier_records_of_a_headless_session() {
        let home = scratch("claude");
        let tool_call = json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "mcp__brigadier__report", "input": {}}]}});
        let sandbox = json!({"type": "attachment", "attachment": {"type": "sandbox_instructions",
            "content": "Network: {\"allowUnixSockets\":[\"/data/x/run/brigadierd.sock\"]}"}});
        transcript(&home, "/data/x/orch/a", A, "sdk-cli", &[tool_call.clone()]);
        transcript(&home, "/data/x/orch/b", B, "sdk-cli", &[sandbox]);
        // Only text that mentions the tools (the user's message above): no mark.
        transcript(&home, "/data/x/orch/c", C, "sdk-cli", &[]);
        // Started in a terminal: never Brigadier's, whatever it says.
        transcript(&home, "/data/x/orch/d", D, "cli", &[tool_call]);
        std::fs::create_dir_all(home.join("todos")).unwrap();
        std::fs::write(home.join("todos").join(format!("{A}-agent-{A}.json")), "[]").unwrap();
        std::fs::create_dir_all(home.join("file-history").join(A)).unwrap();

        let found = find(ProviderKind::Claude, &home, &[home.to_path_buf()], &|_| {
            true
        });
        let mark = |id: &str| {
            found
                .iter()
                .find(|session| session.id == id)
                .unwrap()
                .fingerprint
                .clone()
        };
        assert_eq!(mark(A), Some(Fingerprint::BrigadierTools));
        assert_eq!(
            mark(B),
            Some(Fingerprint::BrigadierSocket {
                data_dir: PathBuf::from("/data/x")
            })
        );
        assert_eq!(mark(C), None);
        assert_eq!(mark(D), None);

        let a = found.iter().find(|session| session.id == A).unwrap();
        let paths: Vec<&Path> = a.entries.iter().map(|entry| entry.path.as_path()).collect();
        assert!(paths.contains(&home.join("file-history").join(A).as_path()));
        assert!(
            paths.contains(
                &home
                    .join("todos")
                    .join(format!("{A}-agent-{A}.json"))
                    .as_path()
            )
        );
        // The project folder last, and only once empty.
        let last = a.entries.last().unwrap();
        assert!(last.empty_only && last.path.ends_with("-data-x-orch-a"));
        assert!(
            a.entries
                .iter()
                .all(|entry| entry.path.starts_with(&entry.root))
        );
    }

    #[test]
    fn only_the_wanted_working_folders_are_read() {
        let home = scratch("wanted");
        transcript(&home, "/data/x/orch/a", A, "sdk-cli", &[]);
        transcript(&home, "/Users/me/code", B, "sdk-cli", &[]);
        let found = find(ProviderKind::Claude, &home, &[], &|cwd| {
            cwd.starts_with("/data/x")
        });
        assert_eq!(
            found.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec![A]
        );
    }

    #[test]
    fn codex_threads_carry_their_originator() {
        let home = scratch("codex");
        let day = home.join("sessions").join("2026").join("10").join("10");
        std::fs::create_dir_all(&day).unwrap();
        for (id, originator) in [("01a1-ours", "brigadier"), ("01a1-user", "codex_exec")] {
            let meta = json!({"type": "session_meta", "payload": {
                "id": id, "cwd": "/data/x/worktrees/p/task-1", "originator": originator}});
            std::fs::write(
                day.join(format!("rollout-2026-10-10T00-00-00-{id}.jsonl")),
                lines(&[meta]),
            )
            .unwrap();
        }
        std::fs::create_dir_all(home.join("generated_images").join("01a1-ours")).unwrap();
        let found = find(ProviderKind::Codex, &home, &[], &|_| true);
        let ours = found.iter().find(|s| s.id == "01a1-ours").unwrap();
        assert_eq!(ours.fingerprint, Some(Fingerprint::Originator));
        assert_eq!(ours.entries.len(), 2, "its rollout and its images");
        let user = found.iter().find(|s| s.id == "01a1-user").unwrap();
        assert_eq!(user.fingerprint, None);
    }
}
