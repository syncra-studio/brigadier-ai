//! What a session may never write (PLAN bug 6): other sessions' branches, the branches its
//! work lands on, and, at Full access, Brigadier's bookkeeping and other sessions' temp
//! folders. Derived from what the engine already knows; the OS sandbox enforces it on every
//! path a CLI can take (its shell, scripts, `eval`), so no command is parsed.

use std::path::{Path, PathBuf};

use brigadier_sandbox::Protection;

use super::SessionManager;
use crate::model::{ConversationId, Environment, Setup};

/// Where every engine-made branch lives: other sessions' task and session branches.
const ENGINE_BRANCHES: &str = "brigadier";

/// The refs a session on `own` may not move in the repository whose shared git folder is
/// `common`: every engine branch but its own, and each of `targets` (the branches its work
/// lands on, which only the engine moves). Below Full access `packed-refs` is protected too;
/// at Full access it stays writable, so the user's own branch work isn't broken, and the
/// ref check catches a hand edit.
pub(crate) fn ref_protection(
    common: &Path,
    own: Option<&str>,
    targets: &[String],
    full: bool,
) -> Protection {
    let heads = common.join("refs/heads");
    let logs = common.join("logs/refs/heads");
    let mut protection = Protection {
        deny: vec![heads.join(ENGINE_BRANCHES), logs.join(ENGINE_BRANCHES)],
        ..Protection::default()
    };
    for target in targets {
        if Some(target.as_str()) != own && !protection.deny.contains(&heads.join(target)) {
            protection.deny.push(heads.join(target));
            protection.deny.push(logs.join(target));
        }
    }
    if !full {
        protection.deny.push(common.join("packed-refs"));
    }
    if let Some(own) = own {
        let reference = heads.join(own);
        let mut lock = reference.clone().into_os_string();
        lock.push(".lock");
        protection
            .allow
            .extend([reference, PathBuf::from(lock), logs.join(own)]);
    }
    protection
}

impl SessionManager {
    /// The branches a session's work lands on, which only the engine moves: a worktree
    /// session's base and its own branch, a local checkout's picked branch, an overnight
    /// run's branch.
    pub(crate) fn landing_targets(&self, id: &ConversationId) -> Vec<String> {
        let mut targets = Vec::new();
        if let Ok(conversation) = self.core.conversation(id)
            && let Some(Setup::Session { environment, .. }) = &conversation.setup
        {
            match environment {
                Environment::LocalCheckout { branch } => targets.push(branch.clone()),
                Environment::NewWorktree { base, branch, .. } => {
                    targets.extend([base.clone(), branch.clone()]);
                }
            }
        }
        if let Some(run) = self
            .overnight
            .active
            .get(id)
            .and_then(|active| active.workspace)
            && !targets.contains(&run.branch)
        {
            targets.push(run.branch);
        }
        targets
    }

    /// What a session working on `own` in the checkout at `checkout` may never write
    /// ([`ref_protection`]); empty when `checkout` isn't a repository.
    pub(crate) fn session_protection(
        &self,
        id: &ConversationId,
        checkout: &Path,
        own: Option<&str>,
        extra_targets: &[String],
        full: bool,
    ) -> Protection {
        let Ok(repo) = self.git.open(checkout) else {
            return Protection::default();
        };
        let mut targets = self.landing_targets(id);
        for target in extra_targets {
            if !targets.contains(target) {
                targets.push(target.clone());
            }
        }
        ref_protection(repo.common_dir(), own, &targets, full)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worker_may_move_only_its_own_branch() {
        let common = Path::new("/r/.git");
        let protection = ref_protection(
            common,
            Some("brigadier/aa/task-1"),
            &["main".into(), "brigadier/aa/session".into()],
            false,
        );
        let paths = |list: &[PathBuf]| {
            list.iter()
                .map(|path| path.strip_prefix(common).unwrap().display().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            paths(&protection.deny),
            [
                "refs/heads/brigadier",
                "logs/refs/heads/brigadier",
                "refs/heads/main",
                "logs/refs/heads/main",
                "refs/heads/brigadier/aa/session",
                "logs/refs/heads/brigadier/aa/session",
                "packed-refs",
            ]
        );
        assert_eq!(
            paths(&protection.allow),
            [
                "refs/heads/brigadier/aa/task-1",
                "refs/heads/brigadier/aa/task-1.lock",
                "logs/refs/heads/brigadier/aa/task-1",
            ]
        );
        assert!(protection.deny_patterns.is_empty());
    }

    #[test]
    fn the_thread_may_move_its_session_branch_and_at_full_access_packed_refs() {
        let common = Path::new("/r/.git");
        let protection = ref_protection(
            common,
            Some("feature"),
            &["main".into(), "feature".into()],
            true,
        );
        assert!(!protection.deny.contains(&common.join("refs/heads/feature")));
        assert!(protection.deny.contains(&common.join("refs/heads/main")));
        assert!(!protection.deny.contains(&common.join("packed-refs")));
        assert!(
            protection
                .allow
                .contains(&common.join("refs/heads/feature"))
        );
    }
}
