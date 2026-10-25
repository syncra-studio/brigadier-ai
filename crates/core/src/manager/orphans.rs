//! The orphan sweep (D3): what sessions this data folder deleted left behind before Delete
//! took all of it, removed at launch and from Storage's cleanup. Only what this data folder's
//! own records prove its own goes: a deleted conversation its catalog still names, a folder in
//! its own sessions folder no conversation has, a CLI session it ran in one. A conversation in
//! the catalog (an archived one too) is never swept, nor a branch outside a deleted session's
//! namespace (`harness/*`, `main`).
//!
//! Merged or empty branches, clean worktrees, their config and refs, the folder, the temp
//! folder and the CLI files go at once. A branch with work that never landed stays, recorded as
//! kept: Storage › Free up space offers it with the existing "not merged" ask. Once nothing of
//! a deleted session is left, its records go too ([`Artifact::Traces`]).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use brigadier_providers::Artifact;

use super::branch_names::branch_namespace;
use super::{SessionManager, blocking};
use crate::Result;
use crate::model::{
    ConversationId, DomainEvent, Environment, KeptBranch, ProjectId, Setup, streams,
};

/// A deleted session whose leftovers the sweep looks for.
#[derive(Debug, Default)]
struct Deleted {
    /// Its id, when its records still say it (else only its folder's name is known).
    id: Option<ConversationId>,
    short: String,
    /// Repositories it worked in, as far as its records and folders tell.
    repos: BTreeSet<PathBuf>,
    /// The branch its work landed on.
    target: Option<String>,
    project: Option<ProjectId>,
}

/// What the sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct OrphanSweep {
    pub removed_branches: Vec<String>,
    pub kept_branches: Vec<String>,
    pub removed_worktrees: Vec<PathBuf>,
    pub removed_folders: Vec<PathBuf>,
    pub failures: Vec<String>,
}

impl SessionManager {
    /// Removes the leftovers of sessions this data folder deleted (see the module docs).
    pub(crate) async fn sweep_orphans(&self) -> OrphanSweep {
        let mut sweep = OrphanSweep::default();
        let catalog = self.core.catalog();
        let live: HashSet<String> = catalog
            .conversations
            .iter()
            .map(|c| c.id.short().to_owned())
            .collect();
        let mut deleted: BTreeMap<String, Deleted> = BTreeMap::new();
        // Deleted conversations the catalog's records still name.
        match crate::ledger::read_stream(&self.core, streams::CATALOG.into()).await {
            Ok(events) => {
                for event in events {
                    let Ok(decoded) = crate::sessions::decode(&event) else {
                        continue;
                    };
                    let (id, setup, project) = match decoded {
                        DomainEvent::ConversationCreated { conversation } => (
                            conversation.id.clone(),
                            conversation.setup.clone(),
                            conversation.project_id.clone(),
                        ),
                        DomainEvent::ConversationSetUp { id, setup } => (id, Some(setup), None),
                        DomainEvent::ConversationDeleted { id } => (id, None, None),
                        _ => continue,
                    };
                    if catalog.conversations.iter().any(|c| c.id == id) {
                        continue;
                    }
                    let entry = deleted.entry(id.short().to_owned()).or_default();
                    entry.short = id.short().to_owned();
                    entry.id = Some(id);
                    entry.project = entry.project.take().or(project);
                    if let Some(Setup::Session {
                        repo, environment, ..
                    }) = setup
                    {
                        entry.repos.insert(PathBuf::from(repo));
                        entry.target = Some(match environment {
                            Environment::NewWorktree { base, .. } => base,
                            Environment::LocalCheckout { branch } => branch,
                        });
                    }
                }
            }
            Err(err) => sweep.failures.push(format!("catalog: {err}")),
        }
        // Folders in its own sessions folder no conversation has.
        let sessions_dir = self.runtime.platform().paths().sessions_dir.clone();
        for entry in std::fs::read_dir(&sessions_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.len() == 8
                && name.bytes().all(|b| b.is_ascii_hexdigit())
                && entry.path().is_dir()
            {
                let found = deleted.entry(name.clone()).or_default();
                found.short = name;
                found.repos.extend(worktree_repos(&entry.path()));
            }
        }
        deleted.retain(|short, _| !live.contains(short));
        if deleted.is_empty() {
            return sweep;
        }
        // The project repositories too: a deleted session's branches are found by name there.
        let mut repos: BTreeSet<PathBuf> = catalog
            .projects
            .iter()
            .flat_map(|project| project.repos.iter().map(|repo| PathBuf::from(&repo.path)))
            .collect();
        repos.extend(deleted.values().flat_map(|d| d.repos.iter().cloned()));
        // Each repository is read once, for all of them.
        let wanted: Vec<Wanted> = deleted
            .values()
            .map(|found| Wanted {
                probe: ConversationId(format!("00000000-0000-0000-0000-0000{}", found.short)),
                folder: sessions_dir.join(&found.short),
                target: found.target.clone(),
            })
            .collect();
        let git = self.git.clone();
        let swept = blocking(move || {
            Ok(repos
                .iter()
                .filter(|repo| repo.exists())
                .map(|repo| (repo.clone(), sweep_repo(&git, repo, &wanted)))
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
        let ledger = self.runtime.ledger().clone();
        let recorded = self.kept_branches().await.unwrap_or_default();
        // Not yet gone (work kept, a failure): its records stay, and the next sweep looks again.
        let mut unclean: HashSet<String> = HashSet::new();
        let mut spaces: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (repo, result) in swept {
            sweep
                .failures
                .extend(result.failures.iter().map(|(_, why)| why.clone()));
            unclean.extend(result.failures.into_iter().map(|(short, _)| short));
            for (short, space) in result.spaces {
                spaces.entry(short).or_default().insert(space);
            }
            sweep.removed_branches.extend(result.removed);
            sweep.removed_worktrees.extend(result.worktrees);
            let repo_text = repo.display().to_string();
            let mut new = Vec::new();
            for (short, kept) in result.kept {
                unclean.insert(short);
                sweep.kept_branches.push(kept.name.clone());
                // Recorded once: Storage offers what is recorded, with the "not merged" ask.
                if !recorded.iter().any(|(at, was)| {
                    *at == repo_text && was.name == kept.name && was.tip == kept.tip
                }) {
                    new.push(kept);
                }
            }
            if let Err(err) = ledger.record_kept_branches(repo_text, new).await {
                sweep.failures.push(err.to_string());
            }
        }
        let natives = ledger.native_sessions();
        let held: HashSet<String> = ledger
            .owners()
            .into_iter()
            .flat_map(|(_, artifacts, _)| artifacts)
            .filter_map(|artifact| match artifact {
                Artifact::ClaudeSession { session_id, .. } => Some(session_id),
                Artifact::CodexThread { thread_id, .. } => Some(thread_id),
                _ => None,
            })
            .collect();
        let marker = self.runtime.platform().paths().owner_marker();
        let temps: Vec<PathBuf> = std::fs::read_dir("/tmp")
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("brigadier-")
            })
            .map(|entry| entry.path())
            .collect();
        let made = ledger.made_branches();
        let mut gone = Vec::new();
        let mut needles = Vec::new();
        for (short, found) in &deleted {
            let folder = sessions_dir.join(short);
            let owner = format!("orphan:{short}");
            // The CLI sessions that ran in its folder, through their CLIs' own removal.
            for (native, session) in &natives {
                if held.contains(native)
                    || !session
                        .cwd
                        .as_ref()
                        .is_some_and(|cwd| Path::new(cwd).starts_with(&folder))
                {
                    continue;
                }
                let artifact = match session.kind {
                    brigadier_providers::ProviderKind::Claude => Artifact::ClaudeSession {
                        session_id: native.clone(),
                        home: None,
                        cwd: session.cwd.clone(),
                    },
                    brigadier_providers::ProviderKind::Codex => Artifact::CodexThread {
                        thread_id: native.clone(),
                        home: None,
                        cwd: session.cwd.clone(),
                    },
                };
                if let Err(err) = ledger.record(&owner, artifact).await {
                    sweep.failures.push(err.to_string());
                }
            }
            // Its temp folders, those this data folder made.
            for temp in &temps {
                let ours = temp.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .starts_with(&format!("brigadier-{short}-"))
                }) && brigadier_sandbox::removal::read_owner_marker(temp).as_deref()
                    == Some(marker.as_str());
                if ours
                    && let Err(err) = ledger
                        .record(
                            &owner,
                            Artifact::ClaudeTempDir {
                                path: temp.display().to_string(),
                            },
                        )
                        .await
                {
                    sweep.failures.push(err.to_string());
                }
            }
            let left = ledger.dispose(&owner).await;
            sweep.failures.extend(left.failures.iter().cloned());
            let mut clean = left.is_clean() && !unclean.contains(short);
            // What the ledger still keeps for it under its own id.
            if let Some(id) = &found.id {
                let suffix = format!(":{id}");
                for (holder, artifacts, disposing) in ledger.owners() {
                    if disposing || !holder.ends_with(&suffix) {
                        continue;
                    }
                    // Its branches went above, one by one: none goes unasked.
                    for artifact in artifacts
                        .iter()
                        .filter(|a| matches!(a, Artifact::GitBranches { .. }))
                    {
                        if let Err(err) = ledger.unrecord(&holder, artifact.clone()).await {
                            sweep.failures.push(err.to_string());
                        }
                    }
                    // A worktree is Storage's to offer, with its work kept.
                    if artifacts
                        .iter()
                        .any(|a| matches!(a, Artifact::Worktree { .. }))
                    {
                        clean = false;
                        continue;
                    }
                    let left = ledger.dispose(&holder).await;
                    sweep.failures.extend(left.failures.iter().cloned());
                    clean &= left.is_clean();
                }
            }
            // Its folder, once no worktree is left in it.
            if folder.is_dir() {
                if worktree_repos(&folder).is_empty() {
                    match std::fs::remove_dir_all(&folder) {
                        Ok(()) => sweep.removed_folders.push(folder.clone()),
                        Err(err) => {
                            clean = false;
                            sweep.failures.push(format!("{}: {err}", folder.display()));
                        }
                    }
                } else {
                    clean = false;
                }
            }
            // Its own records: what the delete of an older Brigadier left in the database.
            if clean && let Some(id) = &found.id {
                match self.forget_deleted(id, found.project.clone()).await {
                    Ok(tasks) => {
                        gone.push(id.0.clone());
                        gone.extend(tasks);
                        needles.push(folder.display().to_string());
                        needles.extend(spaces.get(short).into_iter().flatten().cloned());
                        // The namespaces it made branches in, gone before this sweep too.
                        needles.extend(
                            made.iter()
                                .filter_map(|branch| branch_namespace(id, branch)),
                        );
                    }
                    Err(err) => sweep.failures.push(format!("{short}: {err}")),
                }
            }
        }
        // Nothing of them left: their records go too, all at once.
        if !gone.is_empty() {
            let traces = crate::ledger::traces_owner("orphans");
            let recorded = ledger
                .record(
                    &traces,
                    Artifact::Traces {
                        conversation: String::new(),
                        ids: gone,
                        needles,
                    },
                )
                .await;
            match recorded {
                Ok(()) => {
                    let left = ledger.dispose(&traces).await;
                    if left.is_clean() {
                        self.scrub_databases().await;
                    }
                    sweep.failures.extend(left.failures);
                }
                Err(err) => sweep.failures.push(err.to_string()),
            }
        }
        if !sweep.failures.is_empty() {
            tracing::warn!(failures = ?sweep.failures, "some leftovers of deleted sessions stay until the next sweep");
        }
        if sweep != OrphanSweep::default() {
            tracing::info!(
                branches = sweep.removed_branches.len(),
                kept = sweep.kept_branches.len(),
                worktrees = sweep.removed_worktrees.len(),
                folders = sweep.removed_folders.len(),
                "removed what deleted sessions left behind"
            );
        }
        sweep
    }

    /// Removes what a deleted conversation left in Brigadier's database, as Delete does: its
    /// and its tasks' streams, Brain entries, routing rows, snapshots and images. Returns its
    /// tasks' ids.
    async fn forget_deleted(
        &self,
        id: &ConversationId,
        project: Option<ProjectId>,
    ) -> Result<Vec<String>> {
        let tasks = self.core.deleted_tasks(id).await?;
        self.forget_routing(id, &tasks).await;
        let mut purge = vec![
            streams::conversation(id),
            streams::orchestrator(id),
            streams::inbox(id),
            streams::draft(&id.to_string()),
        ];
        purge.extend(tasks.iter().map(|task| streams::task(&task.id)));
        self.forget_brain_conversation(id, project.clone()).await;
        for task in &tasks {
            let task = ConversationId(task.id.0.clone());
            self.forget_brain_conversation(&task, project.clone()).await;
            self.delete_turn_snapshots(&task).await?;
        }
        self.delete_turn_snapshots(id).await?;
        self.delete_thread_images(id).await?;
        let store = self.core.store().clone();
        store
            .delete_stream_prefixes(vec![format!("background-output:{id}:")])
            .await?;
        store.delete_streams_and_blobs(purge).await?;
        Ok(tasks.into_iter().map(|task| task.id.0).collect())
    }
}

/// A deleted session as [`sweep_repo`] looks for it.
struct Wanted {
    /// An id with its short id.
    probe: ConversationId,
    folder: PathBuf,
    target: Option<String>,
}

/// What the sweep did in one repository, by short id.
#[derive(Default)]
struct RepoSweep {
    removed: Vec<String>,
    kept: Vec<(String, KeptBranch)>,
    worktrees: Vec<PathBuf>,
    /// Its namespaces found there.
    spaces: Vec<(String, String)>,
    failures: Vec<(String, String)>,
}

/// Sweeps deleted sessions' leftovers in `repo`: the branches in their namespaces (found by
/// short id, whatever prefix they have), the worktrees in their folders or on those branches,
/// their config, and their snapshot refs.
fn sweep_repo(git: &brigadier_git::Git, repo_path: &Path, wanted: &[Wanted]) -> RepoSweep {
    let mut done = RepoSweep::default();
    let repo = match git.open(repo_path) {
        Ok(repo) => repo,
        Err(err) => {
            for one in wanted {
                done.failures.push((
                    one.probe.short().to_owned(),
                    format!("{}: {err}", repo_path.display()),
                ));
            }
            return done;
        }
    };
    let listed = (|| -> brigadier_git::Result<_> {
        Ok((
            repo.read_only(&["for-each-ref", "--format=%(refname:strip=2)", "refs/heads"])?,
            repo.worktrees()?,
            repo.read_only(&["for-each-ref", "--format=%(refname)", "refs/brigadier/s/"])?,
        ))
    })();
    let (branches, worktrees, refs) = match listed {
        Ok(listed) => listed,
        Err(err) => {
            for one in wanted {
                done.failures.push((
                    one.probe.short().to_owned(),
                    format!("{}: {err}", repo_path.display()),
                ));
            }
            return done;
        }
    };
    let configured = config_branches(&repo);
    let main = repo.root().to_path_buf();
    let mut pruned = false;
    for one in wanted {
        let short = one.probe.short().to_owned();
        let result = (|| -> brigadier_git::Result<()> {
            let spaces: BTreeSet<String> = branches
                .lines()
                .chain(configured.iter().map(String::as_str))
                .filter_map(|branch| branch_namespace(&one.probe, branch))
                .collect();
            // Worktrees: the clean ones go; one with uncommitted work stays.
            for worktree in &worktrees {
                let ours = worktree.path != main
                    && (worktree.path.starts_with(&one.folder)
                        || worktree.branch.as_ref().is_some_and(|branch| {
                            spaces
                                .iter()
                                .any(|space| branch.starts_with(space.as_str()))
                        }));
                if !ours {
                    continue;
                }
                if !worktree.path.exists() {
                    if !pruned {
                        repo.prune_worktrees()?;
                        pruned = true;
                    }
                    continue;
                }
                let dirty = git
                    .open(&worktree.path)
                    .and_then(|tree| tree.state())
                    .map_or(true, |state| !state.dirty_files.is_empty());
                if dirty {
                    done.failures.push((
                        short.clone(),
                        format!("{} stays: it has uncommitted work", worktree.path.display()),
                    ));
                    continue;
                }
                repo.remove_worktree(&worktree.path, false)?;
                done.worktrees.push(worktree.path.clone());
            }
            let landing = one
                .target
                .clone()
                .or_else(|| repo.default_branch().ok().flatten())
                .unwrap_or_else(|| "main".into());
            for space in &spaces {
                done.spaces.push((short.clone(), space.clone()));
                for (name, tip) in repo.branches_in(space)? {
                    // Landed when another branch, outside its namespace, holds its tip.
                    let holders = repo.read_only(&[
                        "for-each-ref",
                        "--contains",
                        &tip.0,
                        "--format=%(refname:strip=2)",
                        "refs/heads",
                    ])?;
                    if holders
                        .lines()
                        .any(|holder| !holder.starts_with(space.as_str()))
                    {
                        match repo.delete_branch(&name, true) {
                            Ok(()) => done.removed.push(name),
                            Err(err) => done
                                .failures
                                .push((short.clone(), format!("{name}: {err}"))),
                        }
                    } else {
                        done.kept.push((
                            short.clone(),
                            KeptBranch {
                                name,
                                target: landing.clone(),
                                tip: tip.0,
                            },
                        ));
                    }
                }
                // Config of branches that are gone.
                for name in repo.branch_config_in(space)? {
                    if repo.branch_tip(&name)?.is_none() {
                        repo.remove_branch_config(&name)?;
                    }
                }
            }
            // Its snapshot refs, by the full id they carry.
            let ids: BTreeSet<&str> = refs
                .lines()
                .filter_map(|r| r.strip_prefix("refs/brigadier/s/")?.split('/').next())
                .filter(|id| id.len() > 8 && id.ends_with(short.as_str()))
                .collect();
            for id in ids {
                repo.delete_session_snapshots(id)?;
            }
            Ok(())
        })();
        if let Err(err) = result {
            done.failures
                .push((short, format!("{}: {err}", repo_path.display())));
        }
    }
    done
}

/// The branches named by `branch.<name>.*` config sections.
fn config_branches(repo: &brigadier_git::Repo) -> Vec<String> {
    repo.read_only(&[
        "config",
        "--local",
        "--name-only",
        "--get-regexp",
        r"^branch\.",
    ])
    .unwrap_or_default()
    .lines()
    .filter_map(|key| Some(key.strip_prefix("branch.")?.rsplit_once('.')?.0.to_owned()))
    .collect()
}

/// The repositories the worktrees in `folder` (its `work/` and its workers') belong to.
fn worktree_repos(folder: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut places = vec![folder.join("work")];
    if let Ok(workers) = std::fs::read_dir(folder.join("workers")) {
        places.extend(workers.flatten().map(|worker| worker.path().join("work")));
    }
    for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        places.push(entry.path());
    }
    for place in places {
        let Ok(link) = std::fs::read_to_string(place.join(".git")) else {
            continue;
        };
        // `gitdir: <repo>/.git/worktrees/<name>`.
        if let Some(repo) = link
            .trim()
            .strip_prefix("gitdir: ")
            .map(PathBuf::from)
            .and_then(|dir| Some(dir.parent()?.parent()?.parent()?.to_path_buf()))
            && !found.contains(&repo)
        {
            found.push(repo);
        }
    }
    found
}
