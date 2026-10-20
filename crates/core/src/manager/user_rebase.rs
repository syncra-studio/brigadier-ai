//! A rebase the user runs in the Source panel that stops on conflicts stays in progress in the
//! session's checkout while the user resolves it, with HEAD detached (#283). The thread hears
//! so, and that it may run git there again once the rebase ended; meanwhile its git commands
//! are refused where Brigadier sees them before they run (its `run` tool, a Claude thread's
//! `Bash` through its tool hook), so none lands on the half-done rebase (#342).
//!
//! The rebase is marked the user's by a file in git's own folder of the rebase
//! ([`USER_MARK`]), which git removes with it however it ends (Continue, Abort, a terminal):
//! the mark outlives a restart of Brigadier, and a rebase of the thread's own has none and is
//! left alone. Before each turn of the thread the mark is looked at again, so the thread hears
//! of a rebase the user ended in a terminal.

use brigadier_git::Repo;

use super::{SessionManager, blocking, git_error};
use crate::model::ConversationId;

/// The file in a rebase's folder that marks it the user's from the Source panel.
const USER_MARK: &str = "brigadier-user-rebase";

/// The user's rebase in a checkout: its branch, and what it is rebased onto.
pub(super) type UserRebase = (Option<String>, String);

/// Marks the rebase in progress in `repo`, if any, the user's.
pub(super) fn mark_users_rebase(repo: &Repo) -> brigadier_git::Result<()> {
    if let Some(dir) = repo.rebase_dir()? {
        std::fs::write(dir.join(USER_MARK), "")?;
    }
    Ok(())
}

/// The user's rebase in progress in `repo`, if any ([`mark_users_rebase`]).
fn users_rebase(repo: &Repo) -> brigadier_git::Result<Option<UserRebase>> {
    match repo.rebase_dir()? {
        Some(dir) if dir.join(USER_MARK).is_file() => {
            Ok(repo.rebasing()?.map(|rebase| (rebase.branch, rebase.onto)))
        }
        _ => Ok(None),
    }
}

impl SessionManager {
    /// The user's rebase in progress in the session's checkout, as the disk has it.
    async fn users_rebase_now(&self, id: &ConversationId) -> Option<UserRebase> {
        let (checkout, _) = self.checkout(id).ok()?;
        let git = self.git.clone();
        blocking(move || {
            git.open(&checkout)
                .and_then(|repo| users_rebase(&repo))
                .map_err(git_error)
        })
        .await
        .unwrap_or_else(|err| {
            tracing::debug!(conversation = %id, error = %err, "could not look for the user's rebase");
            None
        })
    }

    /// The thread of `id` hears what changed of the user's rebase in its checkout since it was
    /// last told: that it stopped there (`now`), or (`aborted` or not) that it ended.
    pub(super) async fn tell_user_rebase(
        &self,
        id: &ConversationId,
        now: Option<UserRebase>,
        aborted: bool,
    ) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        conv.user_rebase_looked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let note = {
            let mut told = conv.user_rebase.lock().unwrap_or_else(|p| p.into_inner());
            let note = match (&now, told.as_ref()) {
                (Some((branch, onto)), None) => Some(started_line(branch.as_deref(), onto)),
                (None, Some((branch, _))) => Some(ended_line(branch.as_deref(), aborted)),
                _ => None,
            };
            *told = now;
            note
        };
        if let Some(note) = note {
            conv.note(note).await;
        }
    }

    /// Before a turn of the thread: the user's rebase it was told of, or one of before
    /// Brigadier started, is looked at on disk again.
    pub(super) async fn look_at_user_rebase(&self, id: &ConversationId) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let told = conv
            .user_rebase
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_some();
        if told
            || !conv
                .user_rebase_looked
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            let now = self.users_rebase_now(id).await;
            self.tell_user_rebase(id, now, false).await;
        }
    }

    /// Why the thread of `id` may not run `command` now: it runs git while the user's rebase
    /// is in progress in the session's checkout.
    pub(crate) async fn user_rebase_refusal(
        &self,
        id: &ConversationId,
        command: &str,
    ) -> Option<String> {
        if !runs_git(command) {
            return None;
        }
        let now = self.users_rebase_now(id).await;
        let refused = now
            .as_ref()
            .map(|(branch, onto)| refusal(branch.as_deref(), onto));
        self.tell_user_rebase(id, now, false).await;
        refused
    }
}

/// Whether a shell command line runs git: one of its words is `git` (or a path to it).
pub(crate) fn runs_git(command: &str) -> bool {
    command
        .split(|c: char| c.is_whitespace() || ";&|()`$\"'{}".contains(c))
        .any(|word| word == "git" || word.ends_with("/git"))
}

fn branch_named(branch: Option<&str>) -> String {
    branch.map_or_else(|| "your branch".to_owned(), |branch| format!("`{branch}`"))
}

/// The thread's note once the user's rebase stopped on conflicts in its checkout.
fn started_line(branch: Option<&str>, onto: &str) -> String {
    format!(
        "[workspace] The user is rebasing {} onto `{onto}` in the Source panel and is resolving its conflicts in your checkout, where HEAD is detached meanwhile. Run no git there (no commit, add, stash, checkout, reset or rebase) until you're told the rebase ended: your git commands are refused till then. Work that needs no git goes on.",
        branch_named(branch)
    )
}

/// The thread's note once the user's rebase in its checkout ended.
fn ended_line(branch: Option<&str>, aborted: bool) -> String {
    let how = if aborted {
        "was aborted, so the branch is as it was before it"
    } else {
        "ended"
    };
    format!(
        "[workspace] The user's rebase of {} in your checkout {how}: git is yours to run there again. Look at the branch (`git log`, `git status`) before you go on.",
        branch_named(branch)
    )
}

/// What a git command of the thread's gets while the user's rebase is in progress.
fn refusal(branch: Option<&str>, onto: &str) -> String {
    format!(
        "Not run: the user is rebasing {} onto `{onto}` in the Source panel and resolving its conflicts in your checkout, where HEAD is detached, so a git command there would land on the half-done rebase. Run no git there until you're told the rebase ended; work that needs no git goes on.",
        branch_named(branch)
    )
}

#[cfg(test)]
mod tests {
    use super::runs_git;

    #[test]
    fn a_command_runs_git_by_its_words() {
        for command in [
            "git commit -am x",
            "cd /w && git status",
            "/usr/bin/git log",
            "(git add .)",
            "echo $(git rev-parse HEAD)",
        ] {
            assert!(runs_git(command), "{command}");
        }
        for command in [
            "cargo test",
            "cat .gitignore",
            "ls .git",
            "gitk",
            "rg git-lfs",
        ] {
            assert!(!runs_git(command), "{command}");
        }
    }
}
