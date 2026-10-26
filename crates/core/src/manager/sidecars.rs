//! Sidecar stand-ins: a Tauri app's build script wants each `bundle.externalBin` program beside
//! its config, named for the target (`binaries/brigadierd-aarch64-apple-darwin`). Those are
//! built or staged, and git-ignored, so a fresh worktree has none, and `cargo clippy` or `cargo
//! build` of the app fails there before it checks a line (SESSION-FIXES-PLAN.md Q6, the
//! 2026-10-11 token A/B). Every worktree Brigadier makes gets an empty stand-in for each, as
//! the project's own full checks make one.
//!
//! - **Only what git ignores:** a stand-in is made only where nothing exists and git ignores
//!   the path, so it can never be committed or shadow a tracked file.
//! - **The host's name:** the stand-in is for the machine's own target, the one a worker builds
//!   for. A cross build still needs its own.
//! - **Bounded and best effort:** configs are looked for [`MAX_DEPTH`] folders down, outside
//!   dependency and build folders; anything unreadable is skipped, and nothing fails the
//!   worktree.

use std::path::{Path, PathBuf};

use brigadier_git::{Git, Repo};

/// How many folders down a `tauri.conf.json` is looked for (`apps/desktop/src-tauri` is 3).
const MAX_DEPTH: usize = 4;
/// Folders never looked into.
const SKIPPED: &[&str] = &["node_modules", "target", "dist", "build", "vendor"];
/// Tauri's JSON config, the one that names the sidecars.
const CONFIG: &str = "tauri.conf.json";
/// This platform's config, which Tauri merges over [`CONFIG`] (its `externalBin` replaces the
/// main one's; Windows and Linux list the computer-use helper there).
const PLATFORM_CONFIG: &str = if cfg!(windows) {
    "tauri.windows.conf.json"
} else if cfg!(target_os = "linux") {
    "tauri.linux.conf.json"
} else {
    "tauri.macos.conf.json"
};

/// [`stand_in`] for the worktree just made at `path`, logged.
pub(crate) fn stand_in_at(git: &Git, path: &Path) {
    match git.open(path) {
        Ok(worktree) => {
            let made = stand_in(&worktree);
            if !made.is_empty() {
                tracing::info!(worktree = %path.display(), ?made, "made sidecar stand-ins");
            }
        }
        Err(err) => {
            tracing::debug!(worktree = %path.display(), %err, "couldn't open the worktree for sidecar stand-ins")
        }
    }
}

/// Makes the empty stand-ins `worktree` lacks; returns their repo-relative paths.
pub(crate) fn stand_in(worktree: &Repo) -> Vec<String> {
    let root = worktree.root().to_owned();
    let mut configs = Vec::new();
    find_configs(&root, 0, &mut configs);
    let mut made = Vec::new();
    for config in configs {
        for program in external_bins(&config) {
            let Some(dir) = config.parent() else {
                continue;
            };
            let path = dir.join(format!("{program}-{}{}", host_triple(), exe_suffix()));
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if path.symlink_metadata().is_ok() || !worktree.is_ignored(&relative).unwrap_or(false) {
                continue;
            }
            let made_it = path
                .parent()
                .is_some_and(|parent| std::fs::create_dir_all(parent).is_ok())
                && std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .is_ok();
            if made_it {
                made.push(relative);
            } else {
                tracing::debug!(path = %path.display(), "couldn't make a sidecar stand-in");
            }
        }
    }
    made
}

/// Every Tauri config under `dir` (the main one and this platform's), at most [`MAX_DEPTH`]
/// folders down.
fn find_configs(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_file() && (name == CONFIG || name == PLATFORM_CONFIG) {
            found.push(entry.path());
        } else if kind.is_dir()
            && depth < MAX_DEPTH
            && !name.starts_with('.')
            && !SKIPPED.contains(&name.as_ref())
        {
            find_configs(&entry.path(), depth + 1, found);
        }
    }
}

/// The `bundle.externalBin` programs of a JSON Tauri config (v2, or v1's `tauri.bundle`).
fn external_bins(config: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(config) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let bins = json
        .pointer("/bundle/externalBin")
        .or_else(|| json.pointer("/tauri/bundle/externalBin"));
    bins.and_then(|bins| bins.as_array())
        .map(|bins| {
            bins.iter()
                .filter_map(|bin| bin.as_str())
                // Stays inside the config's folder tree: no absolute path, no `..`.
                .filter(|bin| {
                    !bin.is_empty()
                        && Path::new(bin)
                            .components()
                            .all(|part| matches!(part, std::path::Component::Normal(_)))
                })
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The machine's own target triple, as Tauri names a sidecar for it.
pub(crate) fn host_triple() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => format!("{arch}-apple-darwin"),
        "windows" => format!("{arch}-pc-windows-msvc"),
        "linux" => format!("{arch}-unknown-linux-gnu"),
        os => format!("{arch}-unknown-{os}"),
    }
}

fn exe_suffix() -> &'static str {
    std::env::consts::EXE_SUFFIX
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use brigadier_git::WorktreeSpec;

    #[test]
    fn a_new_worktree_gets_an_empty_stand_in_for_each_ignored_sidecar() {
        // Canonical (macOS's /var is /private/var), but not on Windows, where that is a \\?\
        // path git reads no config from.
        let temp = std::env::temp_dir();
        let temp = if cfg!(windows) {
            temp
        } else {
            std::fs::canonicalize(temp).unwrap()
        };
        let dir = temp.join(format!(
            "brigadier-sidecars-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("gitconfig");
        std::fs::write(&config, "").unwrap();
        let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
            .collect();
        env.push(("GIT_CONFIG_GLOBAL".into(), config.into_os_string()));
        for (key, value) in [
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ] {
            env.push((key.into(), value.into()));
        }
        let git = Git::new(PathBuf::from("git"), env);
        let root = dir.join("checkout");
        git.init(&root).unwrap();
        let repo = git.open(&root).unwrap();
        let app = root.join("apps/desktop/src-tauri");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            root.join(".gitignore"),
            "/apps/desktop/src-tauri/binaries/\n",
        )
        .unwrap();
        std::fs::write(
            app.join(CONFIG),
            r#"{"bundle": {"externalBin": ["binaries/brigadierd", "tracked/tool", "../outside"]}}"#,
        )
        .unwrap();
        // This platform's config adds one, and repeats one the main config names.
        std::fs::write(
            app.join(PLATFORM_CONFIG),
            r#"{"bundle": {"externalBin": ["binaries/brigadierd", "binaries/brigadier-cu"]}}"#,
        )
        .unwrap();
        // A sidecar git doesn't ignore is left alone: a stand-in could be committed.
        std::fs::create_dir_all(app.join("tracked")).unwrap();
        std::fs::write(app.join("tracked/keep"), "").unwrap();
        let commit = repo.commit_changes("Start", true).unwrap();
        let path = dir.join("worktree");
        repo.add_worktree(&path, WorktreeSpec::Detached { at: commit })
            .unwrap();
        let worktree = git.open(&path).unwrap();

        let sidecar = format!(
            "apps/desktop/src-tauri/binaries/brigadierd-{}{}",
            host_triple(),
            exe_suffix()
        );
        let helper = format!(
            "apps/desktop/src-tauri/binaries/brigadier-cu-{}{}",
            host_triple(),
            exe_suffix()
        );
        let mut made_now = stand_in(&worktree);
        made_now.sort();
        assert_eq!(made_now, [helper, sidecar.clone()]);
        let made = path.join(&sidecar);
        assert_eq!(std::fs::metadata(&made).unwrap().len(), 0);
        // Nothing to commit, and a second call leaves the existing file alone.
        assert!(worktree.is_ignored(&sidecar).unwrap());
        std::fs::write(&made, "real").unwrap();
        assert!(stand_in(&worktree).is_empty());
        assert_eq!(std::fs::read_to_string(&made).unwrap(), "real");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn host_triples_are_named_as_tauri_names_them() {
        let triple = host_triple();
        assert!(triple.starts_with(std::env::consts::ARCH), "{triple}");
        #[cfg(target_os = "macos")]
        assert!(triple.ends_with("-apple-darwin"));
    }
}
