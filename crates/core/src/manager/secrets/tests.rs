use super::*;
use std::fs;

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    tree: PathBuf,
    git: brigadier_git::Git,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "20261017-2108-workspaces-rework-w54-copies-{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut env: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
            .collect();
        let config = root.join("gitconfig");
        fs::write(&config, "").unwrap();
        env.push(("GIT_CONFIG_GLOBAL".into(), config.into_os_string()));
        env.push(("GIT_CONFIG_NOSYSTEM".into(), "1".into()));
        let git = brigadier_git::Git::new("git".into(), env);
        let repo = root.join("repo");
        git.init(&repo).unwrap();
        fs::write(repo.join(".gitignore"), ".env*\nconfig/\nsecrets/\nkeys/\n").unwrap();
        fs::write(repo.join(".worktreeinclude"), "config/\n").unwrap();
        let tree = root.join("tree");
        fs::create_dir(&tree).unwrap();
        Self {
            root,
            repo,
            tree,
            git,
        }
    }

    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn copy(&self, listed: &[String]) -> Result<(Vec<PathBuf>, Vec<String>)> {
        copy_into(&self.git, &self.repo, &self.tree, listed)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn normal_files_and_included_directories_copy_with_redaction() {
    let f = Fixture::new();
    f.write(".env", b"TOKEN=normal-secret-value-123\n");
    f.write("apps/web/.env.local", b"PORT=4321\n");
    f.write("config/nested/local.bin", &[0, 255, 3]);
    f.write("config/empty", b"");
    f.write("keys/token", b"KEY=listed-secret-value-456\n");
    f.write("unlisted.log", b"not included");
    fs::write(f.tree.join(".env"), "old content").unwrap();
    let (copied, values) = f.copy(&["keys/token".into()]).unwrap();
    for file in [
        ".env",
        "apps/web/.env.local",
        "config/nested/local.bin",
        "config/empty",
        "keys/token",
    ] {
        assert!(copied.contains(&PathBuf::from(file)), "{copied:?}");
        assert_eq!(
            fs::read(f.repo.join(file)).unwrap(),
            fs::read(f.tree.join(file)).unwrap()
        );
    }
    assert!(!f.tree.join("unlisted.log").exists());
    assert!(
        values.contains(&"normal-secret-value-123".into()),
        "{values:?}"
    );
    assert!(
        values.contains(&"listed-secret-value-456".into()),
        "{values:?}"
    );
    assert_eq!(fs::read_dir(&f.tree).unwrap().count(), 4);
}

#[cfg(unix)]
#[test]
fn source_links_and_linked_include_directories_are_skipped() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.root.join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("token"), "KEY=outside-secret-value-123\n").unwrap();
    symlink(outside.join("token"), f.repo.join(".env")).unwrap();
    fs::create_dir(f.repo.join("config")).unwrap();
    symlink(outside.join("token"), f.repo.join("config/linked")).unwrap();
    symlink(&outside, f.repo.join("config/dir")).unwrap();
    symlink(&outside, f.repo.join("secrets")).unwrap();
    f.write("config/normal", b"normal");
    let (copied, values) = f.copy(&["secrets/token".into()]).unwrap();
    assert_eq!(copied, [PathBuf::from("config/normal")]);
    assert!(values.is_empty());
    assert!(!f.tree.join(".env").exists());
    assert!(!f.tree.join("config/linked").exists());
    assert!(!f.tree.join("config/dir").exists());
    assert_eq!(
        fs::read_to_string(outside.join("token")).unwrap(),
        "KEY=outside-secret-value-123\n"
    );
}

#[cfg(unix)]
#[test]
fn destination_file_links_are_refused_without_changing_the_referent() {
    use std::os::unix::fs::symlink;
    for file in [".env", "config/nested/local", "keys/token"] {
        let f = Fixture::new();
        f.write(file, b"new secret");
        let outside = f.root.join("outside");
        fs::write(&outside, "untouched").unwrap();
        let target = f.tree.join(file);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        symlink(&outside, &target).unwrap();
        assert!(f.copy(&["keys/token".into()]).is_err(), "{file}");
        assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched");
        assert!(
            fs::symlink_metadata(target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}

#[cfg(unix)]
#[test]
fn destination_parent_links_are_refused_before_creating_or_writing_anything_outside() {
    use std::os::unix::fs::symlink;
    for (file, parent) in [
        ("apps/.env", "apps"),
        ("config/local", "config"),
        ("config/new/deep/local", "config"),
        ("config/nested/local", "config/nested"),
        ("keys/token", "keys"),
    ] {
        let f = Fixture::new();
        f.write(file, b"new secret");
        let outside = f.root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("local"), "untouched").unwrap();
        fs::create_dir_all(f.tree.join(parent).parent().unwrap()).unwrap();
        symlink(&outside, f.tree.join(parent)).unwrap();
        assert!(f.copy(&["keys/token".into()]).is_err(), "{file}");
        assert_eq!(
            fs::read_to_string(outside.join("local")).unwrap(),
            "untouched"
        );
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    }
}

#[cfg(unix)]
#[test]
fn dangling_and_internal_destination_links_are_also_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.write(".env", b"new secret");
    let missing = f.root.join("missing");
    symlink(&missing, f.tree.join(".env")).unwrap();
    assert!(f.copy(&[]).is_err());
    assert!(!missing.exists());
    fs::remove_file(f.tree.join(".env")).unwrap();
    fs::write(f.tree.join("other"), "untouched").unwrap();
    symlink("other", f.tree.join(".env")).unwrap();
    assert!(f.copy(&[]).is_err());
    assert_eq!(
        fs::read_to_string(f.tree.join("other")).unwrap(),
        "untouched"
    );
}

#[cfg(unix)]
#[test]
fn copies_replace_hardlinks_and_preserve_source_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.write(".env", b"new secret");
    fs::set_permissions(f.repo.join(".env"), fs::Permissions::from_mode(0o600)).unwrap();
    let outside = f.root.join("outside");
    fs::write(&outside, "untouched").unwrap();
    fs::hard_link(&outside, f.tree.join(".env")).unwrap();
    f.copy(&[]).unwrap();
    assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched");
    assert_eq!(
        fs::read_to_string(f.tree.join(".env")).unwrap(),
        "new secret"
    );
    assert_eq!(
        fs::metadata(f.tree.join(".env"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[cfg(unix)]
#[tokio::test]
async fn redaction_reads_also_skip_linked_sources() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let outside = f.root.join("outside");
    fs::write(&outside, "KEY=outside-secret-value-123\n").unwrap();
    symlink(&outside, f.repo.join(".env")).unwrap();
    assert!(values(&f.git, &f.repo, &[]).await.is_empty());
}

#[cfg(unix)]
#[test]
fn source_special_files_are_skipped_without_opening_them() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    let f = Fixture::new();
    mkfifo(&f.repo.join(".env"), Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
    let (copied, values) = f.copy(&[]).unwrap();
    assert!(copied.is_empty());
    assert!(values.is_empty());
}

#[test]
fn invalid_relative_paths_are_refused() {
    let f = Fixture::new();
    for file in ["../outside", "/outside", ""] {
        assert!(files::source(&f.repo, Path::new(file)).is_none());
    }
}

#[cfg(unix)]
#[test]
fn a_linked_worktree_root_is_refused() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    f.write(".env", b"new secret");
    let linked = f.root.join("linked-tree");
    symlink(&f.tree, &linked).unwrap();
    assert!(copy_into(&f.git, &f.repo, &linked, &[]).is_err());
    assert_eq!(fs::read_dir(&f.tree).unwrap().count(), 0);
}

#[test]
fn a_destination_directory_is_refused_without_leaving_temporary_files() {
    let f = Fixture::new();
    f.write(".env", b"new secret");
    fs::create_dir(f.tree.join(".env")).unwrap();
    assert!(f.copy(&[]).is_err());
    assert_eq!(fs::read_dir(&f.tree).unwrap().count(), 1);
    assert!(f.tree.join(".env").is_dir());
}
