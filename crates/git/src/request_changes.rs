//! Private request records and file restores. Their temporary indexes never replace the user's.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};

use crate::command::{TempIndex, valid_oid, valid_path};
use crate::{Error, LandOutcome, LandRequest, Oid, Repo, Result, parse};

/// A file's object and executable/symlink mode in a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileBlob {
    /// The object containing its bytes.
    pub oid: String,
    /// Its git tree mode.
    pub mode: String,
}

impl Repo {
    /// Every file in a tree or commit, keyed by its repository-relative path.
    pub fn file_blobs(&self, tree: &Oid) -> Result<BTreeMap<String, FileBlob>> {
        valid_oid(tree)?;
        let bytes = self.cmd(&["ls-tree", "-rz", &tree.0], true)?;
        let mut files = BTreeMap::new();
        for entry in bytes.split(|b| *b == 0).filter(|entry| !entry.is_empty()) {
            let (header, path) = entry.split_at(
                entry
                    .iter()
                    .position(|b| *b == b'\t')
                    .ok_or_else(|| Error::Parse("tree entry has no path".into()))?,
            );
            let fields: Vec<_> = parse::text(header)?.split_whitespace().collect();
            if fields.len() != 3 {
                return Err(Error::Parse("invalid tree entry".into()));
            }
            files.insert(
                parse::text(&path[1..])?.to_owned(),
                FileBlob {
                    mode: fields[0].into(),
                    oid: fields[2].into(),
                },
            );
        }
        Ok(files)
    }

    /// Stores bytes without changing the checkout or index.
    pub fn store_file_blob(&self, bytes: &[u8], mode: &str) -> Result<FileBlob> {
        let oid = parse::oid(&self.git.checked(
            Some(&self.root),
            &["hash-object", "-w", "--stdin"],
            false,
            &[],
            Some(bytes),
        )?)?;
        Ok(FileBlob {
            oid: oid.0,
            mode: mode.to_owned(),
        })
    }

    /// Reads a file from the checkout without following a symlink in its parent paths.
    pub fn checkout_blob(&self, path: &str) -> Result<Option<FileBlob>> {
        let file = self.safe_file(path)?;
        let metadata = match fs::symlink_metadata(&file) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(file)?;
            return self
                .store_file_blob(target.as_os_str().as_encoded_bytes(), "120000")
                .map(Some);
        }
        if !metadata.is_file() {
            return Err(Error::Invalid(format!("{path} is not a file")));
        }
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        self.store_file_blob(
            &fs::read(file)?,
            if executable { "100755" } else { "100644" },
        )
        .map(Some)
    }

    fn safe_file(&self, path: &str) -> Result<std::path::PathBuf> {
        valid_path(path)?;
        if Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || path.split('/').any(|p| p.eq_ignore_ascii_case(".git"))
        {
            return Err(Error::Invalid("unsafe request file path".into()));
        }
        let file = self.root.join(path);
        let mut parent = file.parent();
        while let Some(dir) = parent.filter(|dir| *dir != self.root) {
            if fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(Error::Invalid(format!("{path} has a symlink parent")));
            }
            parent = dir.parent();
        }
        Ok(file)
    }

    fn blob_tree(
        &self,
        base: Option<&Oid>,
        files: &BTreeMap<String, Option<FileBlob>>,
    ) -> Result<Oid> {
        let index = TempIndex::new()?;
        match base {
            Some(base) => {
                valid_oid(base)?;
                self.index_cmd(&index, &["read-tree", &base.0])?;
            }
            None => {
                self.index_cmd(&index, &["read-tree", "--empty"])?;
            }
        }
        let mut input = Vec::new();
        for (path, blob) in files {
            self.safe_file(path)?;
            match blob {
                Some(blob) => {
                    valid_oid(&Oid(blob.oid.clone()))?;
                    if !matches!(blob.mode.as_str(), "100644" | "100755" | "120000") {
                        return Err(Error::Invalid("unsupported file mode".into()));
                    }
                    input.extend_from_slice(
                        format!("{} {}\t{}\0", blob.mode, blob.oid, path).as_bytes(),
                    );
                }
                None => {
                    input.extend_from_slice(format!("0 {}\t{}\0", "0".repeat(40), path).as_bytes())
                }
            }
        }
        self.git.checked(
            Some(&self.root),
            &["update-index", "-z", "--index-info"],
            false,
            &index.env(),
            Some(&input),
        )?;
        parse::oid(&self.index_cmd(&index, &["write-tree"])?)
    }

    /// Retains two sparse trees as a parent/child commit pair under a private ref.
    pub fn retain_change_record(
        &self,
        prefix: &str,
        before: &BTreeMap<String, Option<FileBlob>>,
        after: &BTreeMap<String, Option<FileBlob>>,
        latest: &Oid,
    ) -> Result<Oid> {
        self.change_ref(prefix, "latest", latest)?;
        let before = self.blob_tree(None, before)?;
        let before = self.commit_tree(&before, &[], "Brigadier request before", true)?;
        let after = self.blob_tree(None, after)?;
        let record = self.commit_tree(&after, &[&before], "Brigadier request changes", true)?;
        self.change_ref(prefix, "record", &record)?;
        Ok(record)
    }

    fn change_ref(&self, prefix: &str, name: &str, oid: &Oid) -> Result<()> {
        if !prefix.starts_with("refs/brigadier/changes/") {
            return Err(Error::Invalid("invalid request ref".into()));
        }
        valid_oid(oid)?;
        let name = format!("{prefix}/{name}");
        self.cmd(&["check-ref-format", &name], true)?;
        self.cmd(&["update-ref", &name, &oid.0], false)?;
        Ok(())
    }

    /// Removes only a session's request refs, including the latest snapshots.
    pub fn delete_change_records(&self, conversation: &str) -> Result<()> {
        if conversation.is_empty() || conversation.contains('/') || conversation.contains("..") {
            return Err(Error::Invalid("invalid conversation id".into()));
        }
        let prefix = format!("refs/brigadier/changes/{conversation}/");
        let refs = self.read_only(&["for-each-ref", "--format=%(refname)", &prefix])?;
        for name in refs.lines() {
            self.cmd(&["update-ref", "-d", name], false)?;
        }
        Ok(())
    }

    /// Reconstructs the old file by reverse applying an unclipped unified patch to its new blob.
    pub fn before_patch(
        &self,
        path: &str,
        after: Option<&FileBlob>,
        diff: &str,
    ) -> Result<Option<FileBlob>> {
        let Some(start) = diff.find("@@ ") else {
            return Err(Error::Invalid("patch has no complete hunks".into()));
        };
        let files = BTreeMap::from([(path.to_owned(), after.cloned())]);
        let tree = self.blob_tree(None, &files)?;
        let index = TempIndex::new()?;
        self.index_cmd(&index, &["read-tree", &tree.0])?;
        let new_path = if after.is_some() {
            format!("{:?}", format!("b/{path}"))
        } else {
            "/dev/null".to_owned()
        };
        let patch = format!(
            "--- {:?}\n+++ {}\n{}",
            format!("a/{path}"),
            new_path,
            &diff[start..]
        );
        self.git.checked(
            Some(&self.root),
            &[
                "apply",
                "--cached",
                "--reverse",
                "--unidiff-zero",
                "--whitespace=nowarn",
                "-",
            ],
            false,
            &index.env(),
            Some(patch.as_bytes()),
        )?;
        let before = parse::oid(&self.index_cmd(&index, &["write-tree"])?)?;
        Ok(self.file_blobs(&before)?.remove(path))
    }

    /// Writes only working files. The user's staged versions and index bytes remain intact.
    pub fn restore_file_blobs(&self, files: &BTreeMap<String, Option<FileBlob>>) -> Result<()> {
        let mut writes = Vec::new();
        for (path, blob) in files {
            let file = self.safe_file(path)?;
            let bytes = blob
                .as_ref()
                .map(|blob| {
                    valid_oid(&Oid(blob.oid.clone()))?;
                    self.cmd(&["cat-file", "blob", &blob.oid], true)
                })
                .transpose()?;
            writes.push((file, blob, bytes));
        }
        for (file, blob, bytes) in writes {
            if fs::symlink_metadata(&file).is_ok() {
                fs::remove_file(&file)?;
            }
            if let (Some(blob), Some(bytes)) = (blob, bytes) {
                if let Some(parent) = file.parent() {
                    fs::create_dir_all(parent)?;
                }
                if blob.mode == "120000" {
                    #[cfg(unix)]
                    {
                        use std::os::unix::ffi::OsStrExt;
                        std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(&bytes), &file)?;
                    }
                    #[cfg(not(unix))]
                    return Err(Error::Invalid("symlink restore is unavailable".into()));
                } else {
                    fs::write(&file, bytes)?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(
                            &file,
                            fs::Permissions::from_mode(if blob.mode == "100755" {
                                0o755
                            } else {
                                0o644
                            }),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Builds a restore commit on HEAD and lands it through the normal fast-forward lock.
    pub fn land_file_restore(
        &self,
        branch: &str,
        head: &Oid,
        files: &BTreeMap<String, Option<FileBlob>>,
        message: &str,
    ) -> Result<LandOutcome> {
        let tree = self.blob_tree(Some(head), files)?;
        let commit = self.commit_tree(&tree, &[head], message, true)?;
        self.land(&LandRequest {
            branch: branch.to_owned(),
            expected_tip: head.clone(),
            commit,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Git;
    use std::path::PathBuf;

    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn setup() -> (Scratch, Repo) {
        let dir = std::env::temp_dir().join(format!(
            "20261015-0226-rebuild-wow-w69-git-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let mut env: Vec<_> = std::env::vars_os()
            .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
            .collect();
        for (key, value) in [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.com"),
            ("GIT_COMMITTER_NAME", "Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.com"),
        ] {
            env.push((key.into(), value.into()));
        }
        let git = Git::new("git".into(), env);
        git.init(&dir).unwrap();
        let repo = git.open(&dir).unwrap();
        fs::write(dir.join("a"), "first\n").unwrap();
        repo.commit_changes("Start", true).unwrap();
        (Scratch(dir), repo)
    }

    #[test]
    fn request_changes_git_private_records_survive_gc_and_preserve_head_and_index() {
        let (dir, repo) = setup();
        let head = repo.resolve("HEAD").unwrap();
        fs::write(dir.0.join("a"), "user\n").unwrap();
        repo.cmd(&["add", "a"], false).unwrap();
        let index = fs::read(repo.git_dir().unwrap().join("index")).unwrap();
        let before = repo.checkout_blob("a").unwrap();
        fs::write(dir.0.join("a"), "thread\n").unwrap();
        let after = repo.checkout_blob("a").unwrap();
        let latest = repo.snapshot_files().unwrap();
        let prefix = "refs/brigadier/changes/c/r";
        let record = repo
            .retain_change_record(
                prefix,
                &BTreeMap::from([("a".into(), before.clone())]),
                &BTreeMap::from([("a".into(), after.clone())]),
                &latest,
            )
            .unwrap();
        repo.cmd(&["gc", "--prune=now"], false).unwrap();
        assert_eq!(repo.resolve("HEAD").unwrap(), head);
        assert_eq!(
            fs::read(repo.git_dir().unwrap().join("index")).unwrap(),
            index
        );
        assert_eq!(repo.file_blobs(&record).unwrap().get("a"), after.as_ref());
        let reversed = repo
            .before_patch("a", after.as_ref(), "@@ -1,1 +1,1 @@\n-user\n+thread\n")
            .unwrap();
        assert_eq!(reversed, before);
        repo.restore_file_blobs(&BTreeMap::from([("a".into(), before)]))
            .unwrap();
        assert_eq!(fs::read_to_string(dir.0.join("a")).unwrap(), "user\n");
        assert_eq!(
            fs::read(repo.git_dir().unwrap().join("index")).unwrap(),
            index
        );
        repo.delete_change_records("c").unwrap();
        assert!(
            repo.read_only(&[
                "for-each-ref",
                "--format=%(refname)",
                "refs/brigadier/changes/"
            ])
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn request_changes_reverse_deleted_files_and_refuse_symlink_parents() {
        let (dir, repo) = setup();
        let before = repo.checkout_blob("a").unwrap();
        fs::remove_file(dir.0.join("a")).unwrap();
        assert_eq!(
            repo.before_patch("a", None, "@@ -1,1 +0,0 @@\n-first\n")
                .unwrap(),
            before
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(std::env::temp_dir(), dir.0.join("link")).unwrap();
            assert!(repo.checkout_blob("link/outside").is_err());
            assert!(
                repo.restore_file_blobs(&BTreeMap::from([("link/outside".into(), before)]))
                    .is_err()
            );
        }
    }

    #[test]
    fn request_changes_snapshot_cost_on_checkout() {
        let Some(path) = std::env::var_os("BRIGADIER_SNAPSHOT_BENCH") else {
            return;
        };
        let git = Git::new("git".into(), std::env::vars_os().collect());
        let repo = git.open(Path::new(&path)).unwrap();
        let start = std::time::Instant::now();
        let snapshot = repo.snapshot_files().unwrap();
        eprintln!(
            "request snapshot cost: {:.1} ms ({})",
            start.elapsed().as_secs_f64() * 1000.0,
            snapshot.0
        );
    }
}
