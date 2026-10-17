//! The engine's ref check (PLAN bug 6 step 5), the backstop behind the sandboxes: every branch
//! the engine owns is recorded with its tip and owner, and checked before each landing (the
//! caller: the landing task's worker) and after each thread turn (the caller: the thread).
//! Workers and the thread commit outside the engine, so the check tells their moves from
//! damage: the caller's own branch and any forward move are recorded; a session branch is its
//! thread's to rebase or amend while it is in a turn (#247); any other branch deleted or
//! rewound is restored, and its thread told; the base is never touched, only reported. The
//! check reads the records and runs under the repository's ref lock, which
//! every landing and branch delete takes, and the engine forgets a branch before it deletes
//! it, so the check never undoes the engine's own move.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use brigadier_git::{Oid, Repo};

use super::SessionManager;
use crate::model::{ConversationId, TaskId};

/// Who owns a recorded branch: a task's worker, or the thread of the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recorded {
    pub tip: Oid,
    pub conversation: ConversationId,
    /// The task whose branch it is; `None`: the session's own branch, the thread's.
    pub task: Option<TaskId>,
}

/// Who asked for a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Caller {
    Task(TaskId),
    Thread(ConversationId),
}

impl Caller {
    fn owns(&self, recorded: &Recorded) -> bool {
        match self {
            Self::Task(id) => recorded.task.as_ref() == Some(id),
            Self::Thread(conversation) => {
                recorded.task.is_none() && recorded.conversation == *conversation
            }
        }
    }
}

/// What a conversation's thread is told after a check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Told {
    pub conversation: ConversationId,
    pub text: String,
}

/// One repository's records: its engine branches, and each session's base as last seen.
#[derive(Debug, Clone, Default)]
pub(crate) struct RepoRecords {
    pub branches: BTreeMap<String, Recorded>,
    /// The base a session merges into, the conversation it belongs to and its tip.
    pub bases: BTreeMap<String, (ConversationId, Oid)>,
}

/// Every repository's records, by its shared git folder. In memory: after a restart a
/// branch is recorded again as its owner works ([`SessionManager::record_branch`]).
#[derive(Debug, Default)]
pub(crate) struct RefRecords {
    repos: Mutex<HashMap<PathBuf, RepoRecords>>,
}

impl RefRecords {
    fn with<R>(&self, common: &Path, work: impl FnOnce(&mut RepoRecords) -> R) -> R {
        let mut repos = self.repos.lock().unwrap_or_else(|p| p.into_inner());
        work(repos.entry(common.to_owned()).or_default())
    }
}

/// Runs `rewrite`, the engine's own move of `branch` in `repo` that needn't be forward (a
/// stopped task's work kept without its snapshot or its AI co-authors), under the repository's
/// ref lock, and records the tip it leaves: no check puts the old tip back, before or after
/// the branch's worker is gone.
pub(crate) fn engine_rewrite<T>(
    repo: &Repo,
    store: &RefRecords,
    branch: &str,
    rewrite: impl FnOnce() -> T,
) -> T {
    repo.with_ref_lock(|| {
        let done = rewrite();
        if let Ok(Some(tip)) = repo.branch_tip(branch) {
            store.with(repo.common_dir(), |records| {
                if let Some(recorded) = records.branches.get_mut(branch) {
                    recorded.tip = tip;
                }
            });
        }
        done
    })
}

/// What the check does with one recorded branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Unchanged,
    /// A legitimate move: the new tip is recorded.
    Record(Oid),
    /// A rewrite while its own worker runs (it may have amended): recorded, the old tip told.
    RecordRewrite(Oid),
    /// Deleted, or rewound so the recorded tip is lost: put back.
    Restore,
}

/// The verdict on a branch recorded at `recorded.tip` that now points at `current` (`None`:
/// deleted). `forward`: `current` descends from the recorded tip. `owner_live`: its owner can
/// be moving it now (a task's worker runs; the session's thread is in a turn).
pub(crate) fn judge(
    recorded: &Recorded,
    current: Option<&Oid>,
    forward: bool,
    caller: &Caller,
    owner_live: bool,
) -> Verdict {
    let Some(current) = current else {
        // No session deletes a branch the engine still uses, its own owner included (bug 5).
        return Verdict::Restore;
    };
    if *current == recorded.tip {
        Verdict::Unchanged
    } else if caller.owns(recorded) || forward {
        Verdict::Record(current.clone())
    } else if recorded.task.is_some() && owner_live {
        Verdict::RecordRewrite(current.clone())
    } else if recorded.task.is_none() && owner_live {
        // The session's thread rebased or amended its branch in a turn, seen at a landing's
        // check or another session's before its own turn ended.
        Verdict::Record(current.clone())
    } else {
        Verdict::Restore
    }
}

/// The check on the repository of `repo` for `caller`, its records read and written back
/// under the repository's ref lock: a branch the engine forgot and deleted a moment earlier
/// (a landing's, a removal's) is never recreated from a copy taken before. `live`: the tasks
/// whose worker runs now (its CLI or its terminal); `busy`: the sessions whose thread is in a
/// turn. Restores what it must and returns what each thread is told.
pub(crate) fn check_recorded(
    repo: &Repo,
    store: &RefRecords,
    caller: &Caller,
    live: &HashSet<TaskId>,
    busy: &HashSet<ConversationId>,
) -> Vec<Told> {
    repo.with_ref_lock(|| {
        let common = repo.common_dir().to_owned();
        let mut records = store.with(&common, |records| records.clone());
        if records.branches.is_empty() && records.bases.is_empty() {
            return Vec::new();
        }
        // The folders the protected refs live in, which packing may have pruned.
        for branch in records.branches.keys() {
            for folder in ["refs/heads", "logs/refs/heads"] {
                if let Some(parent) = common.join(folder).join(branch).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
        }
        let told = check_locked(repo, &mut records, caller, live, busy);
        // Recorded tips the check moved; a branch forgotten or recorded meanwhile is left be.
        store.with(&common, |now| {
            for (branch, recorded) in records.branches {
                if let Some(entry) = now.branches.get_mut(&branch)
                    && entry.task == recorded.task
                {
                    entry.tip = recorded.tip;
                }
            }
            for (base, value) in records.bases {
                now.bases.insert(base, value);
            }
        });
        told
    })
}

/// The check itself, on `records` of one repository, its ref lock held by the caller.
fn check_locked(
    repo: &Repo,
    records: &mut RepoRecords,
    caller: &Caller,
    live: &HashSet<TaskId>,
    busy: &HashSet<ConversationId>,
) -> Vec<Told> {
    let mut told = Vec::new();
    for (branch, recorded) in &mut records.branches {
        let current = match repo.branch_tip(branch) {
            Ok(current) => current,
            Err(err) => {
                tracing::warn!(branch = %branch, error = %err, "the ref check could not read a branch");
                continue;
            }
        };
        let forward = current
            .as_ref()
            .is_some_and(|tip| repo.ancestor(&recorded.tip, tip).unwrap_or(false));
        let owner_live = match &recorded.task {
            Some(id) => live.contains(id),
            None => busy.contains(&recorded.conversation),
        };
        match judge(recorded, current.as_ref(), forward, caller, owner_live) {
            Verdict::Unchanged => {}
            Verdict::Record(tip) => recorded.tip = tip,
            Verdict::RecordRewrite(tip) => {
                tracing::info!(branch = %branch, old = %recorded.tip.0, new = %tip.0, "a branch was rewritten while its worker ran");
                told.push(Told {
                    conversation: recorded.conversation.clone(),
                    text: format!(
                        "[{branch} was rewritten while its worker ran: {} → {}]",
                        short(&recorded.tip),
                        short(&tip)
                    ),
                });
                recorded.tip = tip;
            }
            Verdict::Restore => {
                let restored = match &current {
                    None => repo.create_branch(branch, &recorded.tip),
                    Some(now) => repo.reset_branch(branch, &recorded.tip, now),
                };
                let how = if current.is_none() {
                    "it was deleted outside Brigadier"
                } else {
                    "it was moved back outside Brigadier"
                };
                let text = match restored {
                    Ok(()) => {
                        tracing::warn!(branch = %branch, tip = %recorded.tip.0, "the ref check restored a branch");
                        format!("[restored {branch}: {how}]")
                    }
                    Err(err) => format!(
                        "[{branch} could not be restored at {} ({how}): {err}]",
                        short(&recorded.tip)
                    ),
                };
                told.push(Told {
                    conversation: recorded.conversation.clone(),
                    text,
                });
            }
        }
    }
    // The base is the user's branch: never restored, only reported.
    for (base, (conversation, tip)) in &mut records.bases {
        // A base that can't be read (the checkout moved away) is no news.
        let Ok(current) = repo.branch_tip(base) else {
            continue;
        };
        let forward = current
            .as_ref()
            .is_some_and(|now| now == tip || repo.ancestor(tip, now).unwrap_or(false));
        if !forward {
            told.push(Told {
                conversation: conversation.clone(),
                text: match &current {
                    None => format!("[{base}, this session's base, was deleted outside Brigadier. Tell the user; Brigadier doesn't restore it]"),
                    Some(now) => format!(
                        "[{base}, this session's base, was moved back outside Brigadier ({} → {}). Tell the user; Brigadier doesn't restore it]",
                        short(tip),
                        short(now)
                    ),
                },
            });
        }
        if let Some(now) = current {
            *tip = now;
        }
    }
    told
}

fn short(oid: &Oid) -> &str {
    &oid.0[..oid.0.len().min(8)]
}

impl SessionManager {
    /// Tells the thread of `conversation` what the ref check did: it starts a turn, so the
    /// thread can tell the user.
    async fn tell_guard(&self, conversation: &ConversationId, text: String) {
        self.deliver(
            conversation,
            super::conversation::Envelope {
                kind: crate::work::InjectionKind::Reminder,
                label: "branches".into(),
                task_id: None,
                text,
            },
        )
        .await;
    }

    /// Records `branch` in the repository of `checkout` as `task`'s (`None`: the thread's), at
    /// its current tip, unless it is recorded already.
    pub(crate) fn record_branch(
        &self,
        checkout: &Path,
        branch: &str,
        conversation: &ConversationId,
        task: Option<&TaskId>,
    ) {
        let Ok(repo) = self.git.open(checkout) else {
            return;
        };
        let Ok(Some(tip)) = repo.branch_tip(branch) else {
            return;
        };
        // A branch already recorded keeps its record: a resumed owner must not bless damage.
        self.ref_records.with(repo.common_dir(), |records| {
            records
                .branches
                .entry(branch.to_owned())
                .or_insert_with(|| Recorded {
                    tip,
                    conversation: conversation.clone(),
                    task: task.cloned(),
                });
        });
    }

    /// The engine moved `branch` (a rewrite of a task branch as it lands): its tip now is
    /// the recorded one.
    pub(crate) fn moved_branch(&self, checkout: &Path, branch: &str) {
        let Ok(repo) = self.git.open(checkout) else {
            return;
        };
        let Ok(Some(tip)) = repo.branch_tip(branch) else {
            return;
        };
        self.ref_records.with(repo.common_dir(), |records| {
            if let Some(recorded) = records.branches.get_mut(branch) {
                recorded.tip = tip;
            }
        });
    }

    /// Records `base` as the branch the session of `conversation` merges into.
    pub(crate) fn record_base(&self, checkout: &Path, base: &str, conversation: &ConversationId) {
        let Ok(repo) = self.git.open(checkout) else {
            return;
        };
        let Ok(Some(tip)) = repo.branch_tip(base) else {
            return;
        };
        self.ref_records.with(repo.common_dir(), |records| {
            records
                .bases
                .entry(base.to_owned())
                .or_insert((conversation.clone(), tip));
        });
    }

    /// Records the session's own refs in the repository of `checkout`: a worktree session's
    /// branch (the thread's) and its base, a local checkout's branch (the user's: a base), an
    /// overnight run's branch (the thread's).
    pub(crate) fn record_session_refs(&self, conversation: &ConversationId, checkout: &Path) {
        use crate::model::{Environment, Setup};
        if let Ok(record) = self.core.conversation(conversation)
            && let Some(Setup::Session { environment, .. }) = &record.setup
        {
            match environment {
                Environment::LocalCheckout { branch } => {
                    self.record_base(checkout, branch, conversation);
                }
                Environment::NewWorktree { base, branch, .. } => {
                    self.record_branch(checkout, branch, conversation, None);
                    self.record_base(checkout, base, conversation);
                }
            }
        }
        if let Some(run) = self
            .overnight
            .active
            .get(conversation)
            .and_then(|active| active.workspace)
        {
            self.record_branch(checkout, &run.branch, conversation, None);
        }
    }

    /// Forgets `branch` in the repository of `checkout`, before the engine deletes it (so no
    /// check restores it meanwhile). A branch the engine then keeps after all (not merged
    /// yet) is recorded again when its owner next starts.
    pub(crate) fn forget_branch(&self, checkout: &Path, branch: &str) {
        let Ok(repo) = self.git.open(checkout) else {
            return;
        };
        self.ref_records.with(repo.common_dir(), |records| {
            records.branches.remove(branch);
        });
    }

    /// Runs the ref check on the repository of `checkout` for `caller`, and tells each thread
    /// what it must hear.
    pub(crate) async fn ref_check(&self, checkout: &Path, caller: Caller) {
        let Ok(repo) = self.git.open(checkout) else {
            return;
        };
        // The workers that can move their branch now; the registry also holds workers that
        // run nothing (waiting for quota, stopped after dying twice).
        let tasks: Vec<_> = self.tasks_lock().values().cloned().collect();
        let mut live = HashSet::new();
        for task in tasks {
            if task.executing().await {
                live.insert(task.id.clone());
            }
        }
        // The threads in a turn now, which may be rebasing or amending their session branch.
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        let mut busy = HashSet::new();
        for conv in convs {
            if conv.turn_running().await {
                busy.insert(conv.id.clone());
            }
        }
        let store = self.ref_records.clone();
        let told =
            match super::blocking(move || Ok(check_recorded(&repo, &store, &caller, &live, &busy)))
                .await
            {
                Ok(told) => told,
                Err(err) => {
                    tracing::warn!(error = %err, "the ref check failed");
                    return;
                }
            };
        for Told { conversation, text } in told {
            self.tell_guard(&conversation, text).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oid(c: char) -> Oid {
        Oid(c.to_string().repeat(40))
    }

    fn task(n: &str) -> TaskId {
        TaskId(n.to_owned())
    }

    fn conv(n: &str) -> ConversationId {
        ConversationId(n.to_owned())
    }

    /// A scratch repository: `main`, a session branch, two task branches.
    struct Scratch {
        dir: PathBuf,
        repo: Repo,
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl Scratch {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("brigadier-refcheck-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self {
                repo: {
                    let git = |args: &[&str]| run(&dir, args);
                    git(&["init", "-q", "-b", "main"]);
                    git(&["commit", "-q", "--allow-empty", "-m", "Start"]);
                    for branch in [
                        "brigadier/aa/session",
                        "brigadier/aa/task-1",
                        "brigadier/aa/task-2",
                    ] {
                        git(&["branch", branch]);
                    }
                    brigadier_git::Git::new("git".into(), Vec::new())
                        .open(&dir)
                        .unwrap()
                },
                dir,
            }
        }

        fn git(&self, args: &[&str]) -> String {
            run(&self.dir, args)
        }

        /// A new commit on `branch` (forward), without checking it out.
        fn commit_on(&self, branch: &str) -> Oid {
            let tree = self.git(&["rev-parse", &format!("{branch}^{{tree}}")]);
            let commit = self.git(&["commit-tree", &tree, "-p", branch, "-m", "Work"]);
            self.git(&["update-ref", &format!("refs/heads/{branch}"), &commit]);
            Oid(commit)
        }

        fn tip(&self, branch: &str) -> Option<Oid> {
            self.repo.branch_tip(branch).unwrap()
        }

        fn records(&self) -> RepoRecords {
            let mut records = RepoRecords::default();
            let at = |branch: &str| self.tip(branch).unwrap();
            records.branches.insert(
                "brigadier/aa/session".into(),
                Recorded {
                    tip: at("brigadier/aa/session"),
                    conversation: conv("c"),
                    task: None,
                },
            );
            for (branch, id) in [("brigadier/aa/task-1", "t1"), ("brigadier/aa/task-2", "t2")] {
                records.branches.insert(
                    branch.into(),
                    Recorded {
                        tip: at(branch),
                        conversation: conv("c"),
                        task: Some(task(id)),
                    },
                );
            }
            records.bases.insert("main".into(), (conv("c"), at("main")));
            records
        }
    }

    /// The check on `records` alone, under the repository's ref lock.
    fn check_repo(
        repo: &Repo,
        records: &mut RepoRecords,
        caller: &Caller,
        live: &HashSet<TaskId>,
    ) -> Vec<Told> {
        check_busy(repo, records, caller, live, &HashSet::new())
    }

    /// [`check_repo`], with the threads of `busy` in a turn.
    fn check_busy(
        repo: &Repo,
        records: &mut RepoRecords,
        caller: &Caller,
        live: &HashSet<TaskId>,
        busy: &HashSet<ConversationId>,
    ) -> Vec<Told> {
        repo.with_ref_lock(|| check_locked(repo, records, caller, live, busy))
    }

    fn run(cwd: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    #[test]
    fn worker_and_thread_commits_survive_the_check_and_a_deleted_branch_comes_back() {
        let s = Scratch::new();
        let mut records = s.records();
        let live: HashSet<TaskId> = [task("t1"), task("t2")].into();
        // task-1's worker commits and amends; the thread commits on the session branch;
        // task-2's worker commits meanwhile.
        s.commit_on("brigadier/aa/task-1");
        let amended = Oid(s.git(&[
            "commit-tree",
            "brigadier/aa/task-1^{tree}",
            "-p",
            "main",
            "-m",
            "Amended",
        ]));
        s.git(&["update-ref", "refs/heads/brigadier/aa/task-1", &amended.0]);
        let thread = s.commit_on("brigadier/aa/session");
        let other = s.commit_on("brigadier/aa/task-2");
        // Before task-1 lands (its own amend), and after the thread's turn.
        assert!(check_repo(&s.repo, &mut records, &Caller::Task(task("t1")), &live).is_empty());
        assert!(check_repo(&s.repo, &mut records, &Caller::Thread(conv("c")), &live).is_empty());
        assert_eq!(s.tip("brigadier/aa/task-1"), Some(amended.clone()));
        assert_eq!(s.tip("brigadier/aa/session"), Some(thread));
        assert_eq!(s.tip("brigadier/aa/task-2"), Some(other.clone()));
        assert_eq!(records.branches["brigadier/aa/task-1"].tip, amended);
        // Another session's cleanup deletes task-2's live branch: it comes back.
        s.git(&["branch", "-D", "brigadier/aa/task-2"]);
        let told = check_repo(&s.repo, &mut records, &Caller::Thread(conv("c")), &live);
        assert_eq!(s.tip("brigadier/aa/task-2"), Some(other));
        assert_eq!(
            told[0].text,
            "[restored brigadier/aa/task-2: it was deleted outside Brigadier]"
        );
        // A merged, released branch is forgotten before the engine deletes it: it stays gone.
        records.branches.remove("brigadier/aa/task-2");
        s.git(&["branch", "-D", "brigadier/aa/task-2"]);
        assert!(check_repo(&s.repo, &mut records, &Caller::Thread(conv("c")), &live).is_empty());
        assert_eq!(s.tip("brigadier/aa/task-2"), None);
    }

    #[test]
    fn a_session_branch_forced_back_is_restored_and_main_never_touched() {
        let s = Scratch::new();
        let start = s.tip("main").unwrap();
        s.commit_on("brigadier/aa/session");
        s.commit_on("main");
        let mut records = s.records();
        let landed = records.branches["brigadier/aa/session"].tip.clone();
        let live: HashSet<TaskId> = [task("t1")].into();
        // task-1's worker forces the session branch back; the user resets main.
        s.git(&["update-ref", "refs/heads/brigadier/aa/session", &start.0]);
        s.git(&["update-ref", "refs/heads/main", &start.0]);
        let told = check_repo(&s.repo, &mut records, &Caller::Task(task("t1")), &live);
        assert_eq!(s.tip("brigadier/aa/session"), Some(landed));
        assert_eq!(
            s.tip("main"),
            Some(start.clone()),
            "the base is never restored"
        );
        assert!(
            told.iter().any(|t| t.text
                == "[restored brigadier/aa/session: it was moved back outside Brigadier]")
        );
        assert!(told.iter().any(|t| {
            t.text
                .starts_with("[main, this session's base, was moved back")
        }));
        // Reported once: main's new tip is the recorded one.
        assert!(check_repo(&s.repo, &mut records, &Caller::Thread(conv("c")), &live).is_empty());
    }

    /// Issue #247: the thread rebases its session branch, then lands a worker in the same turn.
    /// The landing's check keeps the rebase; another session's branch rewound is still put back.
    #[test]
    fn a_threads_own_rebase_of_its_session_branch_is_kept_and_other_branches_restored() {
        let s = Scratch::new();
        s.commit_on("brigadier/aa/session");
        s.commit_on("brigadier/aa/task-2");
        s.commit_on("main");
        let mut records = s.records();
        let task_2 = records.branches["brigadier/aa/task-2"].tip.clone();
        // The rebase: the session's work onto main's new tip.
        let rebased = Oid(s.git(&[
            "commit-tree",
            "brigadier/aa/session^{tree}",
            "-p",
            "main",
            "-m",
            "Work, rebased",
        ]));
        s.git(&["update-ref", "refs/heads/brigadier/aa/session", &rebased.0]);
        // Meanwhile task-2's branch, its worker gone, is forced back.
        s.git(&["update-ref", "refs/heads/brigadier/aa/task-2", "main~1"]);
        let live: HashSet<TaskId> = [task("t1")].into();
        let busy: HashSet<ConversationId> = [conv("c")].into();
        let told = check_busy(
            &s.repo,
            &mut records,
            &Caller::Task(task("t1")),
            &live,
            &busy,
        );
        assert_eq!(s.tip("brigadier/aa/session"), Some(rebased.clone()));
        assert_eq!(records.branches["brigadier/aa/session"].tip, rebased);
        assert_eq!(s.tip("brigadier/aa/task-2"), Some(task_2));
        assert_eq!(
            told.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["[restored brigadier/aa/task-2: it was moved back outside Brigadier]"]
        );
        // The thread's own check at its turn's end has nothing to add.
        assert!(check_repo(&s.repo, &mut records, &Caller::Thread(conv("c")), &live).is_empty());
        // Deleted, its thread busy or not, the session branch comes back.
        s.git(&["branch", "-D", "brigadier/aa/session"]);
        check_busy(
            &s.repo,
            &mut records,
            &Caller::Task(task("t1")),
            &live,
            &busy,
        );
        assert_eq!(s.tip("brigadier/aa/session"), Some(rebased));
    }

    #[test]
    fn a_rewrite_while_its_worker_runs_is_recorded_and_otherwise_restored() {
        let s = Scratch::new();
        s.commit_on("brigadier/aa/task-2");
        let mut records = s.records();
        let before = records.branches["brigadier/aa/task-2"].tip.clone();
        let amended = Oid(s.git(&[
            "commit-tree",
            "brigadier/aa/task-2^{tree}",
            "-p",
            "main",
            "-m",
            "Amended",
        ]));
        s.git(&["update-ref", "refs/heads/brigadier/aa/task-2", &amended.0]);
        // During task-1's landing check, task-2's worker is live: its amend survives.
        let live: HashSet<TaskId> = [task("t1"), task("t2")].into();
        let told = check_repo(&s.repo, &mut records, &Caller::Task(task("t1")), &live);
        assert_eq!(s.tip("brigadier/aa/task-2"), Some(amended.clone()));
        assert!(
            told[0]
                .text
                .starts_with("[brigadier/aa/task-2 was rewritten while its worker ran:")
        );
        // With its worker gone, a rewrite is damage.
        s.git(&["update-ref", "refs/heads/brigadier/aa/task-2", &before.0]);
        let live: HashSet<TaskId> = [task("t1")].into();
        check_repo(&s.repo, &mut records, &Caller::Task(task("t1")), &live);
        assert_eq!(s.tip("brigadier/aa/task-2"), Some(amended));
    }

    /// At Full access `packed-refs` stays writable; a hand edit deleting a protected ref is
    /// caught by the check (PLAN bug 6, outline-review correction #3).
    #[test]
    fn a_hand_edit_of_packed_refs_is_put_back() {
        let s = Scratch::new();
        let mut records = s.records();
        let tip = s.tip("brigadier/aa/task-2").unwrap();
        s.git(&["pack-refs", "--all"]);
        let packed = s.dir.join(".git/packed-refs");
        let kept: String = std::fs::read_to_string(&packed)
            .unwrap()
            .lines()
            .filter(|line| !line.ends_with("brigadier/aa/task-2"))
            .map(|line| format!("{line}\n"))
            .collect();
        std::fs::write(&packed, kept).unwrap();
        assert_eq!(s.tip("brigadier/aa/task-2"), None);
        check_repo(
            &s.repo,
            &mut records,
            &Caller::Thread(conv("c")),
            &HashSet::new(),
        );
        assert_eq!(s.tip("brigadier/aa/task-2"), Some(tip));
    }

    /// A landing and a check at the same time: the check never undoes the landing's move.
    #[test]
    fn a_check_beside_a_landing_never_undoes_it() {
        let s = Scratch::new();
        let records = std::sync::Arc::new(Mutex::new(s.records()));
        let repo = s.repo.clone();
        let checker = {
            let records = records.clone();
            let repo = repo.clone();
            std::thread::spawn(move || {
                for _ in 0..30 {
                    let mut records = records.lock().unwrap();
                    check_repo(
                        &repo,
                        &mut records,
                        &Caller::Task(task("t2")),
                        &HashSet::new(),
                    );
                }
            })
        };
        let mut last = s.tip("brigadier/aa/session").unwrap();
        for _ in 0..10 {
            let tree = s.git(&["rev-parse", "brigadier/aa/session^{tree}"]);
            let next = Oid(s.git(&["commit-tree", &tree, "-p", &last.0, "-m", "Landed"]));
            let landed = repo
                .land(&brigadier_git::LandRequest {
                    branch: "brigadier/aa/session".into(),
                    expected_tip: last.clone(),
                    commit: next.clone(),
                })
                .unwrap();
            assert!(
                matches!(landed, brigadier_git::LandOutcome::Landed { .. }),
                "{landed:?}"
            );
            last = next;
        }
        checker.join().unwrap();
        let mut records = records.lock().unwrap();
        check_repo(
            &repo,
            &mut records,
            &Caller::Task(task("t2")),
            &HashSet::new(),
        );
        assert_eq!(s.tip("brigadier/aa/session"), Some(last));
    }

    /// A check that starts while a landing forgets and deletes its merged branch reads the
    /// records only once it holds the ref lock: the branch stays deleted.
    #[test]
    fn a_check_waiting_on_a_landing_never_recreates_the_branch_it_deleted() {
        let s = Scratch::new();
        let store = std::sync::Arc::new(RefRecords::default());
        let common = s.repo.common_dir().to_owned();
        store.with(&common, |records| *records = s.records());
        let tip = s.tip("brigadier/aa/task-2").unwrap();
        // The landing holds the ref lock: it forgets the branch, then deletes it.
        s.repo
            .with_ref_lock(|| {
                let checker = {
                    let (repo, store) = (s.repo.clone(), store.clone());
                    std::thread::spawn(move || {
                        check_recorded(
                            &repo,
                            &store,
                            &Caller::Thread(conv("c")),
                            &HashSet::new(),
                            &HashSet::new(),
                        )
                    })
                };
                std::thread::sleep(std::time::Duration::from_millis(200));
                store.with(&common, |records| {
                    records.branches.remove("brigadier/aa/task-2");
                });
                s.git(&["update-ref", "-d", "refs/heads/brigadier/aa/task-2", &tip.0]);
                checker
            })
            .join()
            .map(|told| assert!(told.is_empty(), "{told:?}"))
            .unwrap();
        assert_eq!(s.tip("brigadier/aa/task-2"), None);
    }

    #[test]
    fn own_and_forward_moves_are_recorded_and_damage_restored() {
        let worker = Recorded {
            tip: oid('a'),
            conversation: conv("c"),
            task: Some(task("t1")),
        };
        let session = Recorded {
            tip: oid('a'),
            conversation: conv("c"),
            task: None,
        };
        let me = Caller::Task(task("t1"));
        let other = Caller::Task(task("t2"));
        let thread = Caller::Thread(conv("c"));
        let b = oid('b');
        // Its own amend, before another landing or after the thread's turn.
        assert_eq!(
            judge(&worker, Some(&b), false, &me, true),
            Verdict::Record(b.clone())
        );
        // A concurrent worker's commits, seen at a check made for the thread.
        assert_eq!(
            judge(&worker, Some(&b), true, &thread, true),
            Verdict::Record(b.clone())
        );
        // The thread's commit on the session branch.
        assert_eq!(
            judge(&session, Some(&b), false, &thread, false),
            Verdict::Record(b.clone())
        );
        // A worker, or another session, forcing the session branch back while its thread is
        // in no turn.
        assert_eq!(
            judge(&session, Some(&b), false, &me, false),
            Verdict::Restore
        );
        // The thread's own rebase, seen at a landing's check during its turn.
        assert_eq!(
            judge(&session, Some(&b), false, &me, true),
            Verdict::Record(b.clone())
        );
        // Another branch rewritten while its worker runs: may be its own amend.
        assert_eq!(
            judge(&worker, Some(&b), false, &other, true),
            Verdict::RecordRewrite(b.clone())
        );
        assert_eq!(
            judge(&worker, Some(&b), false, &other, false),
            Verdict::Restore
        );
        // Deleted, even by its own worker (bug 5).
        assert_eq!(judge(&worker, None, false, &me, true), Verdict::Restore);
        assert_eq!(
            judge(&worker, Some(&oid('a')), false, &other, false),
            Verdict::Unchanged
        );
    }
}
