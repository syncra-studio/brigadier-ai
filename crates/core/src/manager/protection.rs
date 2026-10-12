//! What a session may never write (PLAN bug 6): other sessions' branches, the branches its
//! work lands on, and, at Full access, Brigadier's bookkeeping and other sessions' temp
//! folders. Derived from what the engine already knows; the OS sandbox enforces it on every
//! path a CLI can take (its shell, scripts, `eval`), so no command is parsed.

use std::path::{Path, PathBuf};

use brigadier_sandbox::Protection;

use super::SessionManager;
use crate::model::{ConversationId, Environment, Setup};

/// Where every engine-made branch lives: other sessions' task and session branches, and
/// overnight runs' branches.
const ENGINE_BRANCHES: [&str; 2] = ["brigadier", "overnight"];

/// The refs a session on `own` may not move in the repository whose shared git folder is
/// `common`: every engine branch but its own, and each of `targets` (the branches its work
/// lands on, which only the engine moves, and those the engine's records show it made).
/// Below Full access `packed-refs` is protected too; at Full access it stays writable, so the
/// user's own branch work isn't broken, and the ref check catches a hand edit.
///
/// Every other checkout's own git folder (`<common>/worktrees/<name>`: its HEAD and index) is
/// protected as well, `own_git_dir` given back: `git worktree remove` deletes it even when
/// the worktree's files can't be, and `git worktree prune` then drops it, which leaves that
/// session's worktree no repository.
pub(crate) fn ref_protection(
    common: &Path,
    own_git_dir: Option<&Path>,
    own: Option<&str>,
    targets: &[String],
    full: bool,
) -> Protection {
    let heads = common.join("refs/heads");
    let logs = common.join("logs/refs/heads");
    let mut protection = Protection::default();
    for prefix in ENGINE_BRANCHES {
        protection
            .deny
            .extend([heads.join(prefix), logs.join(prefix)]);
    }
    for target in targets {
        if Some(target.as_str()) != own && !protection.deny.contains(&heads.join(target)) {
            protection.deny.push(heads.join(target));
            protection.deny.push(logs.join(target));
        }
    }
    let worktrees = common.join("worktrees");
    protection.deny.push(worktrees.clone());
    if let Some(own) = own_git_dir.filter(|dir| dir.starts_with(&worktrees)) {
        protection.allow.push(own.to_owned());
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

/// What a Full-access thread may also never write: Brigadier's data folder but its own folders
/// (`own`: its scratch folder, its workspace), and every session's temp folder
/// (`/tmp/brigadier-<12 hex>`, a Claude session's `CLAUDE_CODE_TMPDIR`) and task test folder
/// (`/tmp/brigadier-test-…`); the CLI's adapter gives its own temp folder back.
pub(crate) fn add_full_access(protection: &mut Protection, data_dir: &Path, own: &[PathBuf]) {
    protection.deny.push(data_dir.to_owned());
    protection.allow.extend(own.iter().cloned());
    let temp = super::workers::test_data_root();
    let temp = temp.canonicalize().unwrap_or(temp);
    let temp = regex_escape(&temp.display().to_string());
    protection.deny_patterns.extend([
        format!("^{temp}/brigadier-{}(/|$)", "[0-9a-f]".repeat(12)),
        format!("^{temp}/brigadier-test-[^/]+(/|$)"),
    ]);
}

/// Makes the folders the session's own ref and reflog live in: packing refs prunes an emptied
/// one, and git can't make it again inside a protected folder.
pub(crate) fn make_own_folders(protection: &Protection) {
    for own in &protection.allow {
        if let Some(parent) = own.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }
}

/// `text` matched literally inside a regular expression.
fn regex_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\.^$|?*+()[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl SessionManager {
    /// What the thread may never write. At Full access its CLI and its own `run` commands run
    /// inside Brigadier's allow-all-but profile of it; Full-access workers and checks don't
    /// (a Seatbelt profile can't nest, and they build and test), so the ref check is theirs.
    pub(crate) fn thread_protection(
        &self,
        id: &ConversationId,
        workspace: Option<&super::thread::ThreadWorkspace>,
        scratch: &Path,
        full: bool,
    ) -> Protection {
        let mut protection = workspace
            .map(|workspace| {
                self.session_protection(id, &workspace.path, Some(&workspace.branch), &[], full)
            })
            .unwrap_or_default();
        if full {
            let mut own = vec![scratch.to_owned()];
            own.extend(workspace.map(|workspace| workspace.path.clone()));
            add_full_access(&mut protection, &self.data_dir, &own);
        }
        protection
    }

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
        // Every branch Brigadier's records show it made in this repository, as other sessions'
        // branches take Settings › Git's prefix rather than `brigadier/`: those still there
        // and not under an engine prefix already, since the records keep every branch ever
        // made and each adds to the launch's sandbox profile.
        let ownership = self.runtime.ledger().branch_ownership();
        let mut made = ownership.made_in(checkout);
        if let Some(main) = repo.common_dir().parent() {
            made.extend(ownership.made_in(main));
        }
        made.retain(|name| {
            !ENGINE_BRANCHES
                .iter()
                .any(|prefix| name.starts_with(&format!("{prefix}/")))
                && matches!(repo.branch_tip(name), Ok(Some(_)))
        });
        for target in extra_targets.iter().chain(&made) {
            if !targets.contains(target) {
                targets.push(target.clone());
            }
        }
        let own_git_dir = repo.git_dir().ok();
        let protection = ref_protection(
            repo.common_dir(),
            own_git_dir.as_deref(),
            own,
            &targets,
            full,
        );
        make_own_folders(&protection);
        protection
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn git(cwd: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// The incident (PLAN bug 6): a Full-access thread's cleanup deleted another session's
    /// branch, every task's test folder and its own temp folder. Under the thread's real
    /// wrap each of those commands fails, while its own work and the rest of the machine
    /// stay like the user's own terminal.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_full_access_threads_cleanup_touches_nothing_of_another_session() {
        let hex = || uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
        let dir = Temp(std::env::temp_dir().join(format!("brigadier-protect-{}", hex())));
        let repo = dir.0.join("repo");
        let data = dir.0.join("data");
        let scratch = data.join("orch/conv");
        let worktree = data.join("worktrees/conv");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(data.join("brigadier.db"), "db").unwrap();
        std::fs::create_dir_all(data.join("worktrees/other/src")).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "Start"]);
        git(&repo, &["branch", "brigadier/bb/task-1"]);
        git(&repo, &["branch", "overnight/2026-10-11-run-bb"]);
        let worktree_arg = worktree.display().to_string();
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "brigadier/aa/session",
                &worktree_arg,
            ],
        );
        // Another session's worktree: what git keeps of it in the shared `.git` is its own.
        let other_worktree = data.join("worktrees/bb-task-2");
        let other_worktree_arg = other_worktree.display().to_string();
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "brigadier/bb/task-2",
                &other_worktree_arg,
            ],
        );
        // Other sessions' temp and test folders, the thread's own temp folder, and the user's.
        let other_temp = Temp(PathBuf::from(format!("/tmp/brigadier-{}", hex())));
        let other_test = Temp(PathBuf::from(format!("/tmp/brigadier-test-{}", hex())));
        let own_temp = Temp(PathBuf::from(format!("/tmp/brigadier-{}", hex())));
        let mine = Temp(PathBuf::from(format!("/tmp/my-scratch-{}", hex())));
        for folder in [&other_temp, &other_test, &mine] {
            std::fs::create_dir_all(folder.0.join("sub")).unwrap();
        }
        std::fs::create_dir_all(own_temp.0.join("claude-501/proj/session")).unwrap();

        let common = repo.join(".git").canonicalize().unwrap();
        let own_git_dir = common.join("worktrees/conv");
        let mut protection = ref_protection(
            &common,
            Some(own_git_dir.as_path()),
            Some("brigadier/aa/session"),
            &["main".into(), "brigadier/aa/session".into()],
            true,
        );
        add_full_access(&mut protection, &data, &[scratch.clone(), worktree.clone()]);
        // As the adapter gives back the CLI's own temp folder.
        protection.allow.push(own_temp.0.clone());
        protection.keep.push(own_temp.0.clone());
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.0.join("platform")),
        })
        .unwrap();
        let sh = |script: &str| {
            let spec = brigadier_sandbox::SpawnSpec {
                program: PathBuf::from("/bin/sh"),
                args: vec!["-c".into(), script.into()],
                cwd: Some(worktree.clone()),
                ..Default::default()
            };
            let spec = platform.sandbox().protect(spec, &protection).unwrap();
            let out = std::process::Command::new(&spec.program)
                .args(&spec.args)
                .current_dir(&worktree)
                .output()
                .unwrap();
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        };
        let other = git(&repo, &["rev-parse", "brigadier/bb/task-1"]);
        let base = git(&repo, &["rev-parse", "main"]);

        let (ok, stderr) = sh("git branch -D brigadier/bb/task-1");
        assert!(
            !ok && stderr.contains("Operation not permitted"),
            "{stderr}"
        );
        let (ok, _) = sh(
            "git update-ref refs/heads/main $(git -c user.name=t -c user.email=t@example.invalid commit-tree 'HEAD^{tree}' -m Other)",
        );
        assert!(!ok, "the base moves only by the engine");
        git(&repo, &["pack-refs", "--all"]);
        let (ok, _) = sh("git branch -D brigadier/bb/task-1");
        assert!(!ok, "a packed ref's delete fails too");
        assert_eq!(git(&repo, &["rev-parse", "brigadier/bb/task-1"]), other);
        assert_eq!(git(&repo, &["rev-parse", "main"]), base);
        let (ok, _) = sh("git branch -D overnight/2026-10-11-run-bb");
        assert!(!ok, "an overnight run's branch is the engine's too");

        // Removing another session's worktree fails, and so does dropping what git keeps of
        // it: unprotected, `remove` deleted it though the files stayed, and `prune` agreed.
        let (ok, _) = sh(&format!("git worktree remove --force {other_worktree_arg}"));
        assert!(!ok);
        let _ = sh("git worktree prune");
        assert_eq!(
            git(&other_worktree, &["branch", "--show-current"]),
            "brigadier/bb/task-2"
        );

        let (ok, _) = sh(&format!(
            "rm -rf {} {} {}",
            other_temp.0.display(),
            other_test.0.display(),
            own_temp.0.display()
        ));
        assert!(!ok);
        assert!(other_temp.0.join("sub").is_dir() && other_test.0.join("sub").is_dir());
        assert!(own_temp.0.join("claude-501/proj/session").is_dir());
        // Brigadier's bookkeeping and another conversation's worktree.
        let (ok, _) = sh(&format!(
            "rm -rf {0}/brigadier.db {0}/worktrees/other",
            data.display()
        ));
        assert!(!ok);
        assert!(data.join("brigadier.db").exists() && data.join("worktrees/other/src").is_dir());

        // Its own work: a commit on its session branch (packing pruned the folder its ref
        // lives in, which the engine makes again whenever it looks), its scratch, the user's
        // folders.
        make_own_folders(&protection);
        let (ok, stderr) = sh(
            "echo a > f && git add f && git -c user.name=t -c user.email=t@example.invalid commit -q -m Work",
        );
        assert!(ok && stderr.is_empty(), "{stderr}");
        let (ok, stderr) = sh(&format!(
            "echo s > {0}/note && rm {0}/note && rm -rf {1}",
            scratch.display(),
            mine.0.display()
        ));
        assert!(ok, "{stderr}");
        assert!(!mine.0.exists());
    }

    /// A Full-access thread on the user's own checkout: its workspace, given back whole, holds
    /// the shared `.git`, and another session's branch in it still can't be deleted, while
    /// a commit on the user's branch works.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_full_access_thread_on_the_users_checkout_cant_delete_another_sessions_branch() {
        let dir = Temp(std::env::temp_dir().join(format!(
            "brigadier-protect-local-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        )));
        let repo = dir.0.join("repo");
        let data = dir.0.join("data");
        let scratch = data.join("orch/conv");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "Start"]);
        git(&repo, &["branch", "brigadier/bb/task-1"]);
        let common = repo.join(".git").canonicalize().unwrap();
        let mut protection = ref_protection(
            &common,
            Some(common.as_path()),
            Some("main"),
            &["main".into()],
            true,
        );
        add_full_access(&mut protection, &data, &[scratch, repo.clone()]);
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.0.join("platform")),
        })
        .unwrap();
        let sh = |script: &str| {
            let spec = brigadier_sandbox::SpawnSpec {
                program: PathBuf::from("/bin/sh"),
                args: vec!["-c".into(), script.into()],
                cwd: Some(repo.clone()),
                ..Default::default()
            };
            let spec = platform.sandbox().protect(spec, &protection).unwrap();
            let out = std::process::Command::new(&spec.program)
                .args(&spec.args)
                .current_dir(&repo)
                .output()
                .unwrap();
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        };
        let other = git(&repo, &["rev-parse", "brigadier/bb/task-1"]);
        let (ok, stderr) = sh("git branch -D brigadier/bb/task-1");
        assert!(
            !ok && stderr.contains("Operation not permitted"),
            "{stderr}"
        );
        assert_eq!(git(&repo, &["rev-parse", "brigadier/bb/task-1"]), other);
        let (ok, stderr) = sh(
            "echo a > f && git add f && git -c user.name=t -c user.email=t@example.invalid commit -q -m Work",
        );
        assert!(ok && stderr.is_empty(), "{stderr}");
    }

    #[test]
    fn a_worker_may_move_only_its_own_branch() {
        let common = Path::new("/r/.git");
        let protection = ref_protection(
            common,
            Some(common.join("worktrees/task-1").as_path()),
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
                "refs/heads/overnight",
                "logs/refs/heads/overnight",
                "refs/heads/main",
                "logs/refs/heads/main",
                "refs/heads/brigadier/aa/session",
                "logs/refs/heads/brigadier/aa/session",
                "worktrees",
                "packed-refs",
            ]
        );
        assert_eq!(
            paths(&protection.allow),
            [
                "worktrees/task-1",
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
            None,
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
