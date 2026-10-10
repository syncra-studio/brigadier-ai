//! Build files in idle sessions: the build output git ignores (`node_modules`, cargo's
//! `target`, …) in the work folders of sessions that are still open but haven't been used for
//! a while. All of it is rebuilt by the next build; the session keeps everything else.
//!
//! Only folders in the data directory's own work folders are looked at, never the user's
//! checkout. A folder goes only while git still ignores it and tracks nothing in it, its
//! session is still idle and nothing runs in the work folder: all checked again before it goes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use brigadier_sandbox::removal::{self, Bound};

use super::{Cleaned, Records, ScanContext, Scanner, counted, item, unused};
use crate::manager::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, Environment, Lifecycle, Setup};
use crate::storage::{CleanCategory, KeptLine};
use crate::{Error, Result};

/// How long a session must have been unused before its build files go.
pub const IDLE: Duration = Duration::from_secs(3 * 24 * 60 * 60);
/// How deep in a work folder build output is looked for.
const DEPTH: usize = 4;
/// Smaller build output isn't worth listing.
const MIN_BYTES: u64 = 1024 * 1024;
/// Folders that hold only what tools rebuild.
const NAMES: &[&str] = &[
    "node_modules",
    ".next",
    ".nuxt",
    ".turbo",
    ".parcel-cache",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".gradle",
];
/// A folder with a `CACHEDIR.TAG` starting with this is a cache by its own say (cargo's
/// `target`, and others following <https://bford.info/cachedir/>).
const CACHEDIR_SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";

/// Build output in one work folder, as it was listed.
#[derive(Debug, Clone)]
pub struct BuildFiles {
    conversation: ConversationId,
    worktree: PathBuf,
    /// Each folder, bound inside the work folder, with its path in it (`/`-separated) and size.
    entries: Vec<(Bound, String, u64)>,
}

/// A work folder an open session uses.
struct InUse {
    conversation: ConversationId,
    path: PathBuf,
    label: String,
}

/// The work folders in the data directory that open sessions and their running workers use.
fn in_use(records: &Records) -> Vec<InUse> {
    let ours = records.data_dir.join("worktrees");
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    for conversation in &records.conversations {
        if conversation.lifecycle == Lifecycle::Archived
            || conversation.deleting
            || conversation.cleanup_pending
        {
            continue;
        }
        let mut add = |path: &str, label: String| {
            let path = PathBuf::from(path);
            if path.starts_with(&ours) && seen.insert(path.clone()) {
                found.push(InUse {
                    conversation: conversation.id.clone(),
                    path,
                    label,
                });
            }
        };
        if let Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) = &conversation.setup
        {
            add(path, format!("Build files in “{}”", conversation.title));
        }
        for task in records.tasks.get(&conversation.id).into_iter().flatten() {
            if task.state.is_final() {
                continue;
            }
            if let Some(path) = task
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.worktree.as_deref())
            {
                add(
                    path,
                    format!(
                        "Build files of task-{} in “{}”",
                        task.number, conversation.title
                    ),
                );
            }
        }
    }
    found
}

/// Whether `conversation` hasn't been used for `idle`: no turn, task change or running work.
fn idle_for(records: &Records, conversation: &ConversationId, idle: Duration) -> bool {
    if records.running.contains(conversation) {
        return false;
    }
    let Some(found) = records.conversation(&conversation.0) else {
        return false;
    };
    let last = records
        .tasks
        .get(conversation)
        .into_iter()
        .flatten()
        .map(|task| task.updated_at_ms)
        .fold(found.updated_at_ms, i64::max);
    let idle_ms = i64::try_from(idle.as_millis()).unwrap_or(i64::MAX);
    crate::now_ms().saturating_sub(last) >= idle_ms
}

/// Folders in `worktree` (up to [`DEPTH`] down, links and `.git` never entered) that hold only
/// build output by their name or their `CACHEDIR.TAG`. Never looks inside one it found.
fn candidates(worktree: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(worktree.to_owned(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            // The entry's own type: a link is never a folder here.
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = entry.file_name();
            if name == ".git" {
                continue;
            }
            let path = entry.path();
            if NAMES.iter().any(|known| name == *known) || cache_tagged(&path) {
                found.push(path);
            } else if depth + 1 < DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }
    found.sort();
    found
}

fn cache_tagged(dir: &Path) -> bool {
    use std::io::Read as _;
    let mut head = [0u8; CACHEDIR_SIGNATURE.len()];
    std::fs::File::open(dir.join("CACHEDIR.TAG"))
        .and_then(|mut file| file.read_exact(&mut head))
        .is_ok_and(|()| head == CACHEDIR_SIGNATURE)
}

/// `path` inside `worktree`, `/`-separated, as git names it.
fn relative(worktree: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(worktree).ok()?;
    let parts: Option<Vec<&str>> = rel
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect();
    Some(parts?.join("/"))
}

/// Whether git ignores `rel` and tracks nothing in it.
fn only_ignored(repo: &brigadier_git::Repo, rel: &str) -> std::result::Result<(), String> {
    match (repo.is_ignored(rel), repo.tracks_under(rel)) {
        (Ok(true), Ok(false)) => Ok(()),
        (Ok(false), _) => Err(format!("{rel}: git no longer ignores it")),
        (_, Ok(true)) => Err(format!("{rel}: git tracks files in it now")),
        (Err(err), _) | (_, Err(err)) => Err(format!("{rel}: git couldn't tell ({err})")),
    }
}

fn days(idle: Duration) -> String {
    let days = idle.as_secs() / (24 * 60 * 60);
    if days >= 1 {
        counted(days as usize, "day", "days")
    } else {
        "a while".into()
    }
}

impl Scanner<'_> {
    /// Build output in the work folders of sessions idle for `idle`; that of sessions used more
    /// recently is counted as kept.
    pub(super) fn build_files(&mut self, idle: Duration) {
        let mut recent = (0usize, 0u64);
        for used in in_use(self.records) {
            let Ok(repo) = self.git.open(&used.path) else {
                continue;
            };
            let mut entries = Vec::new();
            for path in candidates(&used.path) {
                let Some(rel) = relative(&used.path, &path) else {
                    continue;
                };
                if only_ignored(&repo, &rel).is_err() {
                    continue;
                }
                let bytes = removal::allocated_size(&path);
                if bytes < MIN_BYTES {
                    continue;
                }
                entries.push((path, rel, bytes));
            }
            if entries.is_empty() {
                continue;
            }
            let total: u64 = entries.iter().map(|(_, _, bytes)| bytes).sum();
            if !idle_for(self.records, &used.conversation, idle) {
                recent.0 += 1;
                recent.1 += total;
                continue;
            }
            if self.busy(&used.path) {
                continue;
            }
            let mut names: Vec<String> = entries
                .iter()
                .filter_map(|(path, _, _)| Some(path.file_name()?.to_string_lossy().into_owned()))
                .collect();
            names.sort();
            names.dedup();
            let bound: Vec<(Bound, String, u64)> = entries
                .into_iter()
                .filter_map(|(path, rel, bytes)| {
                    Some((removal::bind(&used.path, &path).ok()?, rel, bytes))
                })
                .collect();
            if bound.is_empty() {
                continue;
            }
            let bytes = bound.iter().map(|(_, _, bytes)| bytes).sum();
            self.push(
                item(
                    CleanCategory::BuildFiles,
                    used.label,
                    Some(used.path.clone()),
                    bytes,
                    &format!(
                        "Build output git ignores ({}); the next build makes it again. The \
                         session hasn't been used for {}, and its work stays.",
                        names.join(", "),
                        days(idle)
                    ),
                    true,
                ),
                super::Action::DeleteBuildFiles(BuildFiles {
                    conversation: used.conversation,
                    worktree: used.path,
                    entries: bound,
                }),
            );
        }
        if recent.0 > 0 {
            self.kept.push(KeptLine {
                category: CleanCategory::BuildFiles,
                label: counted(recent.0, "session's build files", "sessions' build files"),
                bytes: recent.1,
                reason: format!("Their sessions were used in the last {}.", days(idle)),
            });
        }
    }
}

impl SessionManager {
    /// Deletes listed build output after checking again that its session is still open, still
    /// uses that work folder and is still idle, nothing runs in the work folder, and git still
    /// ignores each folder and tracks nothing in it.
    pub(super) async fn delete_build_files(
        &self,
        files: BuildFiles,
        context: ScanContext,
    ) -> Result<Cleaned> {
        let records = self.storage_records().await?;
        let idle = context.build_idle.unwrap_or(IDLE);
        let still_used = in_use(&records)
            .iter()
            .any(|used| used.conversation == files.conversation && used.path == files.worktree);
        if !still_used {
            return Err(Error::Invalid(
                "its session no longer uses that work folder".into(),
            ));
        }
        if !idle_for(&records, &files.conversation, idle) {
            return Err(Error::Invalid("its session was used since the scan".into()));
        }
        let platform = self.runtime.platform().clone();
        let git = self.git.clone();
        blocking(move || {
            unused(&*platform, &files.worktree).map_err(Error::Invalid)?;
            let repo = git.open(&files.worktree).map_err(git_error)?;
            let mut cleaned = Cleaned::default();
            for (bound, rel, bytes) in &files.entries {
                if let Err(why) = only_ignored(&repo, rel) {
                    cleaned.failures.push(why);
                    continue;
                }
                match removal::delete(bound) {
                    Ok(()) => cleaned.reclaimed += bytes,
                    Err(err) => cleaned.failures.push(err.to_string()),
                }
            }
            Ok(cleaned)
        })
        .await
    }
}
