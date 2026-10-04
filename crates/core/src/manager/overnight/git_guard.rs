//! An overnight run's git guard (PLAN.md §10.8): git itself refuses what the run may not do
//! in the run's repository, whatever binary or spelling a worker uses.
//!
//! A run worker's environment includes [`GUARD_CONFIG`] for the run's repository only (git's
//! conditional include on its git folder, so a project's own tests in temporary repositories
//! are untouched). In it:
//!
//! - a `reference-transaction` hook refuses every change to a branch, tag or stash except the
//!   worker's own branch ([`OWNED_REFS_ENV`]);
//! - every push URL is rewritten to a transport that doesn't exist, and a `pre-push` hook
//!   refuses what gets past that.
//!
//! The repository's own hooks still run: each of Brigadier's hooks runs the hook of the same
//! name from where the repository keeps its hooks.
//!
//! What it can't hold: a push to a remote with its own `pushurl` with `--no-verify`, and a
//! worker that overrides the git configuration on purpose. The approval route and the command
//! gate judge those commands first.

use std::path::{Path, PathBuf};

/// The worker's own refs, separated by spaces.
pub(crate) const OWNED_REFS_ENV: &str = "BRIGADIER_OWNED_REFS";

/// The configuration file included for the run's repository.
const GUARD_CONFIG: &str = "guard.config";

/// The client-side hooks git runs, each one Brigadier's dispatcher.
const HOOKS: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "post-rewrite",
    "reference-transaction",
    "pre-auto-gc",
    "sendemail-validate",
    "post-index-change",
];

/// Every hook: the guard's own check, then the repository's hook of the same name.
const DISPATCH: &str = r#"#!/bin/sh
# Brigadier's git guard for an overnight run's workers (PLAN.md 10.8). The repository's own
# hook of the same name runs after it.
hook=$(basename "$0")
input=
if [ "$hook" = reference-transaction ]; then
  input=$(cat)
  if [ "$1" = prepared ]; then
    refused=
    while read -r old new ref; do
      [ -n "$ref" ] || continue
      case "$ref" in
        refs/worktree/*|refs/bisect/*|refs/rewritten/*|refs/remotes/*) continue ;;
        refs/*) ;;
        *) continue ;;
      esac
      owned=
      for mine in $BRIGADIER_OWNED_REFS; do
        [ "$ref" = "$mine" ] && owned=1
      done
      [ -n "$owned" ] || refused="$refused $ref"
    done <<INPUT
$input
INPUT
    if [ -n "$refused" ]; then
      echo "Brigadier: an overnight run's worker changes only its own branch (${BRIGADIER_OWNED_REFS:-none}). Declined by the overnight rules:$refused" >&2
      exit 1
    fi
  fi
fi
if [ "$hook" = pre-push ]; then
  echo "Brigadier: an overnight run never pushes for the user. Declined by the overnight rules." >&2
  exit 1
fi
own=$(GIT_CONFIG_COUNT=0 git config --type=path --get core.hooksPath 2>/dev/null)
[ -n "$own" ] || own="$(git rev-parse --git-common-dir)/hooks"
here=$(cd "$(dirname "$0")" && pwd -P)
[ -d "$own" ] && [ "$(cd "$own" && pwd -P)" = "$here" ] && exit 0
if [ -x "$own/$hook" ]; then
  if [ "$hook" = reference-transaction ]; then
    printf '%s\n' "$input" | "$own/$hook" "$@"
    exit $?
  fi
  exec "$own/$hook" "$@"
fi
exit 0
"#;

/// The first characters a remote URL or path can start with: rewriting each makes every push
/// URL point nowhere.
const URL_STARTS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789/.~_-[\\";

/// Writes the hooks and the configuration under `dir` (only what changed, so a worker running
/// one meanwhile never sees it half-written), and returns the configuration's path.
pub(crate) fn install(dir: &Path) -> std::io::Result<PathBuf> {
    let hooks = dir.join("hooks");
    std::fs::create_dir_all(&hooks)?;
    for hook in HOOKS {
        write_if_changed(&hooks.join(hook), DISPATCH)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(hooks.join(hook), std::fs::Permissions::from_mode(0o755))?;
        }
    }
    let mut config = format!(
        "# Brigadier's git guard for an overnight run's workers (PLAN.md 10.8).\n[core]\n\thooksPath = {}\n[url \"brigadier-push-declined::\"]\n",
        quoted(&git_path(&hooks.canonicalize()?))
    );
    for start in URL_STARTS.chars() {
        config.push_str(&format!(
            "\tpushInsteadOf = {}\n",
            quoted(&start.to_string())
        ));
    }
    let path = dir.join(GUARD_CONFIG);
    write_if_changed(&path, &config)?;
    Ok(path)
}

/// The environment that applies the guard (`config`, from [`install`]) to the repository whose
/// git folder is `common_dir`, for a worker owning `owned` refs, after `before` other
/// environment configuration entries. Returns the entries and the new count.
pub(crate) fn env(
    config: &Path,
    common_dir: &Path,
    owned: &[String],
    before: usize,
) -> (Vec<(String, String)>, usize) {
    // Case-insensitive where the file system is.
    let condition = if cfg!(any(windows, target_os = "macos")) {
        "gitdir/i"
    } else {
        "gitdir"
    };
    let dir = git_path(common_dir);
    let dir = dir.trim_end_matches('/');
    let config = git_path(config);
    let mut env = Vec::new();
    // The repository itself, and every worktree's git folder inside it.
    for (n, pattern) in [dir.to_owned(), format!("{dir}/**")]
        .into_iter()
        .enumerate()
    {
        let at = before + n;
        env.push((
            format!("GIT_CONFIG_KEY_{at}"),
            format!("includeIf.{condition}:{pattern}.path"),
        ));
        env.push((format!("GIT_CONFIG_VALUE_{at}"), config.clone()));
    }
    env.push((OWNED_REFS_ENV.to_owned(), owned.join(" ")));
    (env, before + 2)
}

/// A path as git configuration writes it: forward slashes, no `\\?\` prefix.
fn git_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    text.replace('\\', "/")
}

/// A configuration value in quotes, `"` and `\` escaped.
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn write_if_changed(path: &Path, content: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|now| now == content) {
        return Ok(());
    }
    let staged = path.with_extension("new");
    std::fs::write(&staged, content)?;
    std::fs::rename(&staged, path)
}

#[cfg(all(test, unix))]
mod tests {
    use std::process::{Command, Output};

    use super::*;

    /// A user's repository with a remote, a pre-commit hook of its own and another branch, a
    /// run worker's worktree on `task-1`, and an unrelated repository.
    struct Place {
        base: PathBuf,
        config: PathBuf,
    }

    impl Place {
        fn new(name: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("brigadier-guard-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&base).unwrap();
            let base = base.canonicalize().unwrap();
            let config = install(&base.join("guard")).unwrap();
            let place = Self { base, config };
            place.plain(&["init", "-q", "--bare", "remote.git"], "");
            place.plain(&["init", "-q", "-b", "main", "repo"], "");
            std::fs::write(
                place.base.join("repo/.git/hooks/pre-commit"),
                "#!/bin/sh\ntouch \"$(git rev-parse --git-common-dir)/own-hook-ran\"\n",
            )
            .unwrap();
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    place.base.join("repo/.git/hooks/pre-commit"),
                    std::fs::Permissions::from_mode(0o755),
                )
                .unwrap();
            }
            for args in [
                &["commit", "-q", "--allow-empty", "-m", "init"][..],
                &["branch", "other"],
                &["tag", "v0"],
                &["remote", "add", "origin", "../remote.git"],
                &["push", "-q", "origin", "main"],
                &["worktree", "add", "-q", "../worktree", "-b", "task-1"],
            ] {
                assert!(place.plain(args, "repo").status.success(), "{args:?}");
            }
            place.plain(&["init", "-q", "-b", "main", "unrelated"], "");
            place
        }

        fn git(&self, guarded: bool, program: &str, args: &[&str], dir: &str) -> Output {
            let mut command = Command::new(program);
            command
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(self.base.join(dir))
                .env_remove("GIT_DIR")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_COUNT", "0");
            if guarded {
                let common = self.base.join("repo/.git");
                let (env, count) = env(&self.config, &common, &["refs/heads/task-1".to_owned()], 0);
                command.envs(env).env("GIT_CONFIG_COUNT", count.to_string());
            }
            command.output().unwrap()
        }

        fn guarded(&self, args: &[&str], dir: &str) -> Output {
            self.git(true, "git", args, dir)
        }

        fn plain(&self, args: &[&str], dir: &str) -> Output {
            self.git(false, "git", args, dir)
        }

        fn tip(&self, reference: &str) -> String {
            let out = self.plain(&["rev-parse", reference], "repo");
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }
    }

    impl Drop for Place {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn refused(out: &Output) -> bool {
        !out.status.success()
    }

    #[test]
    fn a_run_worker_changes_only_its_own_branch() {
        let place = Place::new("refs");
        let (main, other) = (place.tip("main"), place.tip("other"));
        // Its own branch: commits, amends, resets, with the repository's own hook running.
        assert!(
            place
                .guarded(&["commit", "-q", "--allow-empty", "-m", "mine"], "worktree")
                .status
                .success()
        );
        assert!(place.base.join("repo/.git/own-hook-ran").exists());
        assert!(
            place
                .guarded(&["reset", "-q", "--hard", "HEAD~1"], "worktree")
                .status
                .success()
        );
        assert!(
            place
                .guarded(&["checkout", "-q", "--detach"], "worktree")
                .status
                .success()
        );
        assert!(
            place
                .guarded(&["checkout", "-q", "task-1"], "worktree")
                .status
                .success()
        );
        // Everything else, from the worktree or the user's checkout.
        std::fs::write(place.base.join("repo/staged.txt"), "x").unwrap();
        assert!(place.plain(&["add", "staged.txt"], "repo").status.success());
        for (args, dir) in [
            (&["branch", "-D", "other"][..], "worktree"),
            (&["branch", "-d", "other"], "worktree"),
            (&["branch", "-f", "other", "HEAD"], "worktree"),
            (&["branch", "new"], "worktree"),
            (&["tag", "v1"], "worktree"),
            (&["tag", "-d", "v0"], "worktree"),
            (&["update-ref", "refs/heads/main", "HEAD"], "worktree"),
            (&["update-ref", "-d", "refs/heads/other"], "worktree"),
            (&["commit", "-q", "--allow-empty", "-m", "x"], "repo"),
            (&["stash", "-q"], "repo"),
        ] {
            let out = place.guarded(args, dir);
            assert!(refused(&out), "{args:?} in {dir}");
        }
        let out = place.guarded(&["branch", "-D", "other"], "worktree");
        assert!(String::from_utf8_lossy(&out.stderr).contains("Declined by the overnight rules"));
        assert_eq!(place.tip("main"), main);
        assert_eq!(place.tip("other"), other);
        assert!(!place.tip("v0").is_empty());
        // An unrelated repository is the worker's own business (a project's tests make some).
        for args in [
            &["commit", "-q", "--allow-empty", "-m", "a"][..],
            &["branch", "x"],
            &["branch", "-D", "x"],
            &["tag", "t"],
        ] {
            assert!(
                place.guarded(args, "unrelated").status.success(),
                "{args:?}"
            );
        }
    }

    #[test]
    fn a_run_worker_never_pushes() {
        let place = Place::new("push");
        let remote = place.base.join("remote.git");
        let remote = remote.to_str().unwrap();
        let before = place.git(false, "git", &["rev-parse", "main"], "remote.git");
        let git = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let absolute = String::from_utf8_lossy(&git.stdout).trim().to_owned();
        for program in ["git", absolute.as_str()] {
            for args in [
                &["push", "origin", "task-1"][..],
                &["push", "--no-verify", "origin", "task-1:main"],
                &["push", "--no-verify", remote, "task-1"],
                &["push", "--force", "origin", "HEAD:main"],
                &[
                    "-c",
                    "remote.origin.url=file:///nowhere",
                    "push",
                    "origin",
                    "task-1",
                ],
            ] {
                let out = place.git(true, program, args, "worktree");
                assert!(refused(&out), "{program} {args:?}");
            }
        }
        let after = place.git(false, "git", &["rev-parse", "main"], "remote.git");
        assert_eq!(before.stdout, after.stdout);
        let branches = place.git(false, "git", &["branch", "--list"], "remote.git");
        assert!(!String::from_utf8_lossy(&branches.stdout).contains("task-1"));
        // Fetching still works.
        assert!(
            place
                .guarded(&["fetch", "-q", "origin"], "worktree")
                .status
                .success()
        );
    }
}
