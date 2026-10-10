//! Agent session files: the Claude Code and Codex session files (transcripts, per-session
//! state, logs) Brigadier's sessions left in the CLIs' homes that its cleanup ledger doesn't
//! hold (sessions from before it recorded them, or of a data directory that is gone).
//!
//! Ownership is proven, never guessed from a name. A session's files are offered only when
//! they ran in one of Brigadier's working places (a data directory's `orch`, `chat`,
//! `worker-home` or `scratch` folder, or one of its work folders), and:
//!
//! - in this data directory: Brigadier recorded starting the session, or its files carry
//!   Brigadier's mark ([`leftovers::Fingerprint`]); and the conversation, task or work folder it served is
//!   gone (deleted, archived, ended);
//! - in another data directory: its files carry Brigadier's mark (naming that data directory
//!   when the mark names one), and that whole data directory is gone.
//!
//! Each file is bound when it is listed. Removing goes through the cleanup ledger, which takes
//! on exactly those entries with their identities ([`Artifact::Adopted`]): what changed since
//! the scan stays, and a removal cut off is finished by the next launch's sweep.
//! A Codex thread goes by its id through Codex's own `thread/delete` (its rollout checked
//! first to still be the file shown), so it also leaves the Codex app's lists (Recents).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use brigadier_providers::leftovers;
use brigadier_providers::{Artifact, ProviderKind};
use brigadier_sandbox::removal::{self, Bound};

use super::{
    Action, Cleaned, OwnerState, Recorded, Records, Scanner, artifact_path, counted, item, unused,
};
use crate::manager::{SessionManager, blocking};
use crate::model::Lifecycle;
use crate::storage::{CleanCategory, KeptLine};
use crate::{Error, Result};

/// Files changed more recently than this may belong to a session still being set up.
const MIN_AGE: Duration = Duration::from_secs(60 * 60);

/// Where one CLI keeps its files.
#[derive(Debug, Clone)]
pub struct AgentHome {
    pub kind: ProviderKind,
    /// Its main home, which holds the history every account shares.
    pub history: PathBuf,
    /// Every home with per-session state of its own: the main home and each extra account's.
    pub homes: Vec<PathBuf>,
}

/// CLI session files proven Brigadier's, as they were listed.
#[derive(Debug, Clone)]
pub struct Adoption {
    sessions: Vec<Session>,
    entries: Vec<Entry>,
    /// Why they are Brigadier's, in plain words.
    evidence: String,
}

/// A listed file or folder.
#[derive(Debug, Clone)]
struct Entry {
    bound: Bound,
    /// Goes only once empty (a project folder other sessions may share).
    empty_only: bool,
    bytes: u64,
    /// A Codex thread's rollout: the thread goes through Codex's own `thread/delete`, which
    /// also takes it off the Codex app's lists, while the file is still the one shown.
    thread: Option<String>,
}

/// One listed session: what it served, checked again before its files go.
#[derive(Debug, Clone)]
struct Session {
    id: String,
    cwd: PathBuf,
    owner: Owner,
}

/// What a session's working place belonged to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Owner {
    /// A conversation of this data directory (`orch`, `chat`, `worker-home`).
    Conversation(String),
    /// A worker's task of this data directory (`scratch`).
    Task(String),
    /// One of this data directory's work folders.
    Worktree(PathBuf),
    /// Another data directory, as a whole.
    DataDir(PathBuf),
}

/// Where a session ran, by Brigadier's layout of a data directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    root: PathBuf,
    owner: Owner,
}

/// The working place `cwd` is in: within this data directory (`data`, as given and resolved)
/// read from its own layout; elsewhere from the nearest folder laid out like one.
fn place(cwd: &Path, data: &[PathBuf]) -> Option<Place> {
    for root in data {
        if let Ok(rel) = cwd.strip_prefix(root) {
            let parts: Vec<&str> = rel
                .components()
                .filter_map(|part| part.as_os_str().to_str())
                .collect();
            return in_layout(root, &parts);
        }
    }
    for dir in cwd.ancestors() {
        let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
            continue;
        };
        let Some(area) = parent.file_name().and_then(|area| area.to_str()) else {
            continue;
        };
        let Some(root) = parent.parent() else {
            continue;
        };
        if matches!(area, "orch" | "chat" | "worker-home" | "scratch") {
            return in_layout(root, &[area, name.to_str()?]);
        }
        if root.file_name() == Some("worktrees".as_ref()) {
            let root = root.parent()?;
            return in_layout(root, &["worktrees", area, name.to_str()?]);
        }
    }
    None
}

fn in_layout(root: &Path, parts: &[&str]) -> Option<Place> {
    let owner = match parts {
        ["orch" | "chat" | "worker-home", id, ..] => Owner::Conversation((*id).to_owned()),
        ["scratch", id, ..] => Owner::Task((*id).to_owned()),
        ["worktrees", project, folder, ..] => {
            Owner::Worktree(root.join("worktrees").join(project).join(folder))
        }
        _ => return None,
    };
    Some(Place {
        root: root.to_owned(),
        owner,
    })
}

/// The data directory as given and as resolved through links (`/tmp` is `/private/tmp`).
fn data_paths(data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![data_dir.to_owned()];
    if let Ok(real) = data_dir.canonicalize()
        && real != data_dir
    {
        paths.push(real);
    }
    paths
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || (a
            .canonicalize()
            .ok()
            .is_some_and(|a| b.canonicalize().ok() == Some(a)))
}

/// CLI sessions the ledger holds (they go with their owner), by id.
fn held_sessions(records: &Records) -> HashSet<String> {
    records
        .owners
        .iter()
        .flat_map(|(_, artifacts, _)| artifacts)
        .filter_map(|artifact| match artifact {
            Artifact::ClaudeSession { session_id, .. } => Some(session_id.clone()),
            Artifact::CodexThread { thread_id, .. } => Some(thread_id.clone()),
            _ => None,
        })
        .collect()
}

/// Paths the ledger holds (adopted entries included).
fn held_paths(records: &Records) -> HashSet<PathBuf> {
    records
        .owners
        .iter()
        .flat_map(|(_, artifacts, _)| artifacts.iter().filter_map(artifact_path))
        .map(PathBuf::from)
        .collect()
}

/// Whether what `owner` names is gone. `live` is the work folders live ledger owners hold.
fn owner_gone(records: &Records, live: &HashSet<String>, owner: &Owner) -> bool {
    let conversation_gone = |id: &str| {
        records.conversation(id).is_none_or(|conversation| {
            conversation.lifecycle == Lifecycle::Archived || conversation.deleting
        })
    };
    match owner {
        Owner::Conversation(id) => conversation_gone(id),
        Owner::Task(id) => match records.task(id) {
            Some((conversation, task)) => {
                task.state.is_final() || conversation_gone(&conversation.id.0)
            }
            None => true,
        },
        Owner::Worktree(path) => {
            !live.contains(&path.display().to_string())
                && !matches!(records.recorded_worktree(path), Recorded::Open)
        }
        Owner::DataDir(root) => removal::is_gone(root),
    }
}

/// Work folders live ledger owners hold.
fn live_worktrees(
    records: &Records,
    states: &std::collections::HashMap<String, OwnerState>,
) -> HashSet<String> {
    records
        .owners
        .iter()
        .filter(|(owner, _, _)| states.get(owner) == Some(&OwnerState::Live))
        .flat_map(|(_, artifacts, _)| artifacts)
        .filter_map(|artifact| match artifact {
            Artifact::Worktree { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

/// Sessions found for one CLI and one data directory.
#[derive(Default)]
struct Group {
    sessions: Vec<Session>,
    entries: Vec<Entry>,
    evidence: Vec<&'static str>,
}

impl Scanner<'_> {
    /// Brigadier's CLI session files the ledger doesn't hold, in `homes`.
    pub(super) fn agent_files(&mut self, homes: &[AgentHome]) {
        let data = data_paths(&self.records.data_dir);
        let held = held_sessions(self.records);
        let held_paths = held_paths(self.records);
        let live = live_worktrees(self.records, self.states);
        let mut seen = HashSet::new();
        let mut groups: BTreeMap<(&'static str, PathBuf), Group> = BTreeMap::new();
        let mut open = (0usize, 0u64);
        for home in homes {
            let wanted = |cwd: &Path| place(cwd, &data).is_some();
            for found in leftovers::find(home.kind, &home.history, &home.homes, &wanted) {
                if !seen.insert((home.kind, found.id.clone())) || held.contains(&found.id) {
                    continue;
                }
                let Some(cwd) = found.cwd.clone() else {
                    continue;
                };
                let Some(place) = place(&cwd, &data) else {
                    continue;
                };
                let ours = data.contains(&place.root);
                let marked = |root: &Path| {
                    found
                        .fingerprint
                        .as_ref()
                        .filter(|mark| mark.data_dir().is_none_or(|named| same_dir(named, root)))
                };
                let (evidence, owner) = if ours {
                    let evidence = if self.records.native.contains_key(&found.id) {
                        "Brigadier recorded starting them"
                    } else if let Some(mark) = marked(&self.records.data_dir) {
                        mark.describe()
                    } else {
                        continue;
                    };
                    (evidence, place.owner)
                } else {
                    let Some(mark) = marked(&place.root) else {
                        continue;
                    };
                    (mark.describe(), Owner::DataDir(place.root.clone()))
                };
                let entries: Vec<Entry> = found
                    .entries
                    .iter()
                    .filter(|entry| !held_paths.contains(&entry.path))
                    .filter_map(|entry| {
                        let bound = removal::bind(&entry.root, &entry.path).ok()?;
                        let bytes = if entry.empty_only {
                            0
                        } else {
                            removal::allocated_size(&entry.path)
                        };
                        let rollout = home.kind == ProviderKind::Codex
                            && entry.root.file_name().is_some_and(|part| {
                                part == "sessions" || part == "archived_sessions"
                            });
                        Some(Entry {
                            bound,
                            empty_only: entry.empty_only,
                            bytes,
                            thread: rollout.then(|| found.id.clone()),
                        })
                    })
                    .collect();
                if entries.iter().all(|entry| entry.empty_only) {
                    continue;
                }
                if !owner_gone(self.records, &live, &owner) {
                    if ours {
                        open.0 += 1;
                        open.1 += entries.iter().map(|entry| entry.bytes).sum::<u64>();
                    }
                    continue;
                }
                let settled = found
                    .last_change
                    .and_then(|at| at.elapsed().ok())
                    .is_some_and(|age| age >= MIN_AGE);
                if !settled || (cwd.is_dir() && self.busy(&cwd)) {
                    continue;
                }
                let group = groups
                    .entry((home.kind.label(), place.root.clone()))
                    .or_default();
                if !group.evidence.contains(&evidence) {
                    group.evidence.push(evidence);
                }
                group.sessions.push(Session {
                    id: found.id,
                    cwd,
                    owner,
                });
                group.entries.extend(entries);
            }
        }
        for ((agent, root), group) in groups {
            let ours = data.contains(&root);
            let sessions = group.sessions.len();
            let label = if ours {
                format!(
                    "{agent} files of {}",
                    counted(sessions, "ended session", "ended sessions")
                )
            } else {
                format!(
                    "{agent} files of {} of a removed Brigadier ({})",
                    counted(sessions, "session", "sessions"),
                    root.display()
                )
            };
            let evidence = group.evidence.join("; ");
            let reason = if ours {
                format!(
                    "Brigadier's own ({evidence}), and what they belonged to is gone. Their \
                     conversations can't be opened again."
                )
            } else {
                format!(
                    "Brigadier's own ({evidence}), and the data folder they belonged to is gone."
                )
            };
            let bytes = group.entries.iter().map(|entry| entry.bytes).sum();
            let path = group
                .entries
                .iter()
                .find(|entry| !entry.empty_only)
                .map(|entry| entry.bound.path().to_owned());
            self.push(
                item(CleanCategory::AgentFiles, label, path, bytes, &reason, true),
                Action::Adopt(Adoption {
                    sessions: group.sessions,
                    entries: group.entries,
                    evidence,
                }),
            );
        }
        if open.0 > 0 {
            self.kept.push(KeptLine {
                category: CleanCategory::AgentFiles,
                label: counted(
                    open.0,
                    "agent session of an open conversation",
                    "agent sessions of open conversations",
                ),
                bytes: open.1,
                reason: "Their conversations are still open.".into(),
            });
        }
    }
}

impl SessionManager {
    /// Hands the listed session files to the cleanup ledger, after checking again that no
    /// owner holds their sessions, what they served is still gone and nothing runs where they
    /// ran. The ledger removes exactly the listed entries while they are still what was listed.
    pub(super) async fn adopt(&self, adoption: Adoption) -> Result<Cleaned> {
        let records = self.storage_records().await?;
        let states = self.owner_states(&records);
        let held = held_sessions(&records);
        let live = live_worktrees(&records, &states);
        for session in &adoption.sessions {
            if held.contains(&session.id) {
                return Err(Error::Invalid(
                    "one of its sessions is Brigadier's again".into(),
                ));
            }
            if !owner_gone(&records, &live, &session.owner) {
                return Err(Error::Invalid(
                    "what one of its sessions belonged to is back".into(),
                ));
            }
        }
        let platform = self.runtime.platform().clone();
        let cwds: Vec<PathBuf> = adoption.sessions.iter().map(|s| s.cwd.clone()).collect();
        blocking(move || {
            for cwd in &cwds {
                unused(&*platform, cwd).map_err(Error::Invalid)?;
            }
            Ok(())
        })
        .await?;
        let mut artifacts = Vec::new();
        let mut changed = Vec::new();
        for entry in &adoption.entries {
            let Some(thread_id) = &entry.thread else {
                artifacts.push(Artifact::Adopted {
                    root: entry.bound.root().display().to_string(),
                    path: entry.bound.path().display().to_string(),
                    identity: entry.bound.identity().parts(),
                    evidence: adoption.evidence.clone(),
                    empty_only: entry.empty_only,
                });
                continue;
            };
            // The thread goes by its id: only while its file is still the one shown.
            match removal::recheck(&entry.bound) {
                Ok(()) => artifacts.push(Artifact::CodexThread {
                    thread_id: thread_id.clone(),
                    home: None,
                    cwd: adoption
                        .sessions
                        .iter()
                        .find(|session| session.id == *thread_id)
                        .map(|session| session.cwd.display().to_string()),
                }),
                Err(err) => changed.push(format!("left in place: {err}")),
            }
        }
        let owner = format!("sweep:{}", uuid::Uuid::now_v7());
        let leftovers = self.runtime.ledger().adopt(&owner, artifacts).await?;
        let reclaimed = adoption
            .entries
            .iter()
            .filter(|entry| removal::is_gone(entry.bound.path()))
            .map(|entry| entry.bytes)
            .sum();
        let mut failures = changed;
        failures.extend(leftovers.failures);
        failures.extend(
            leftovers
                .kept
                .into_iter()
                .map(|why| format!("left in place: {why}")),
        );
        Ok(Cleaned {
            reclaimed,
            trashed: 0,
            failures,
        })
    }
}

#[cfg(test)]
mod place_tests {
    use super::*;

    #[test]
    fn places_follow_the_data_directory_layout() {
        let data = vec![PathBuf::from("/d/brig")];
        let at = |cwd: &str| place(Path::new(cwd), &data);
        assert_eq!(
            at("/d/brig/orch/c1"),
            Some(Place {
                root: "/d/brig".into(),
                owner: Owner::Conversation("c1".into())
            })
        );
        assert_eq!(
            at("/d/brig/worktrees/p/x/chat/deep").map(|p| p.owner),
            Some(Owner::Worktree("/d/brig/worktrees/p/x".into()))
        );
        assert_eq!(at("/d/brig/logs"), None);
        assert_eq!(at("/d/brig"), None);
        assert_eq!(
            at("/gone/other/scratch/t1/sub"),
            Some(Place {
                root: "/gone/other".into(),
                owner: Owner::Task("t1".into())
            })
        );
        assert_eq!(
            at("/gone/other/worktrees/p/x/src").map(|p| p.root),
            Some(PathBuf::from("/gone/other"))
        );
        assert_eq!(at("/Users/me/code/app"), None);
    }
}
