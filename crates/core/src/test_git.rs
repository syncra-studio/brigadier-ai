//! Tests' git: the real binary, never Brigadier's command gate. Inside a Brigadier worker the
//! gate directory (`<data>/gate/bin`) comes first on PATH, and its `git` asks the user before
//! every push, even to a local bare repository a test made.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// This process's PATH without the gate, for a test's git and everything git runs (hooks).
pub(crate) fn path() -> &'static OsStr {
    static PATH: OnceLock<OsString> = OnceLock::new();
    PATH.get_or_init(|| without_gate(&std::env::var_os("PATH").unwrap_or_default()))
}

/// The real git on [`path`].
pub(crate) fn git() -> &'static Path {
    static GIT: OnceLock<PathBuf> = OnceLock::new();
    GIT.get_or_init(|| first_git(path()).expect("git on PATH"))
}

/// `path` without its gate directories: one ending in `gate/bin`, or one whose `git` is a link
/// to brigadierd.
fn without_gate(path: &OsStr) -> OsString {
    let kept = std::env::split_paths(path).filter(|dir| {
        let shim = std::fs::canonicalize(dir.join("git"))
            .is_ok_and(|git| git.file_name() == Some(OsStr::new("brigadierd")));
        !dir.ends_with("gate/bin") && !shim
    });
    std::env::join_paths(kept).expect("PATH entries stay joinable")
}

/// The first executable `git` on `path`.
fn first_git(path: &OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .map(|dir| dir.join("git"))
        .find(|git| {
            std::fs::metadata(git)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_first_on_path_resolves_to_another_git() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("brigadier-test-git-{}", std::process::id()));
        let gate = base.join("data/gate/bin");
        std::fs::create_dir_all(&gate).unwrap();
        let fake = gate.join("git");
        std::fs::write(&fake, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut dirs = vec![gate.clone()];
        dirs.extend(std::env::split_paths(path()));
        let with_gate = std::env::join_paths(dirs).unwrap();
        assert_eq!(first_git(&with_gate).as_deref(), Some(fake.as_path()));
        let real = first_git(&without_gate(&with_gate)).expect("a real git");
        assert_ne!(real, fake);
        assert_eq!(real, git());
        let version = std::process::Command::new(&real)
            .arg("--version")
            .env("PATH", path())
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&version.stdout).starts_with("git version"));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
