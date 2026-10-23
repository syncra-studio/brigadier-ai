//! The main checkout's memory directory, which Claude also creates from a linked worktree.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use brigadier_sandbox::removal::{self, Identity};

use super::files;
use crate::{Artifact, Error, Ledger, Result};

/// Linux stamps a new directory from the kernel's coarse clock, which trails
/// `SystemTime::now()` by up to a tick: one made just after the watch began can read as older.
const CLOCK_SLACK: Duration = Duration::from_secs(1);

pub(super) struct Watch {
    root: PathBuf,
    path: PathBuf,
    since: SystemTime,
    project_absent: bool,
}

impl Watch {
    pub(super) fn start(config: &Path, repo: &Path) -> Option<Self> {
        let project = files::project_dir(config, repo);
        let path = project.join("memory");
        // An unreadable entry is not an absent one. A pre-existing empty folder is the user's.
        if !absent(&path) {
            return None;
        }
        Some(Self {
            root: config.join("projects"),
            path,
            since: SystemTime::now()
                .checked_sub(CLOCK_SLACK)
                .unwrap_or(SystemTime::UNIX_EPOCH),
            project_absent: absent(&project),
        })
    }

    fn artifact(&self) -> Option<Artifact> {
        let bound = removal::bind(&self.root, &self.path).ok()?;
        let meta = std::fs::symlink_metadata(&self.path).ok()?;
        let created = meta.created().ok()?;
        if !meta.is_dir() || created < self.since || created > SystemTime::now() {
            return None;
        }
        if std::fs::read_dir(&self.path).ok()?.next().is_some() {
            return None;
        }
        let project_identity = self
            .project_absent
            .then(|| removal::bind(&self.root, self.path.parent()?).ok())
            .flatten()
            .map(|bound| bound.identity().parts());
        removal::recheck(&bound).ok()?;
        Some(Artifact::ClaudeMemoryDir {
            path: self.path.display().to_string(),
            identity: bound.identity().parts(),
            project_identity,
        })
    }

    pub(super) async fn record(&self, ledger: &dyn Ledger) {
        if let Some(artifact) = self.artifact()
            && let Err(err) = ledger.record(artifact).await
        {
            tracing::warn!(error = %err, "could not record Claude's empty memory folder");
        }
    }
}

fn absent(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_err_and(|err| err.kind() == io::ErrorKind::NotFound)
}

pub(super) fn remove(
    config: &Path,
    path: &Path,
    identity: (u64, u64),
    project_identity: Option<(u64, u64)>,
) -> Result<()> {
    let root = config.join("projects");
    let Some(project) = path.parent().filter(|project| {
        path.file_name().is_some_and(|name| name == "memory") && project.parent() == Some(&root)
    }) else {
        return Err(Error::Invalid(
            "not a Claude project memory directory".into(),
        ));
    };
    remove_empty(&root, path, identity)?;
    if let Some(identity) = project_identity {
        remove_empty(&root, project, identity)?;
    }
    Ok(())
}

fn remove_empty(root: &Path, path: &Path, identity: (u64, u64)) -> Result<()> {
    let bound = match removal::bind_as(root, path, Identity::from_parts(identity)) {
        Ok(bound) => bound,
        // A missing or replaced entry is no longer ours. Leave it alone.
        Err(
            removal::RemovalError::Gone(_)
            | removal::RemovalError::Changed(_)
            | removal::RemovalError::Symlink(_),
        ) => return Ok(()),
        Err(err) => return Err(Error::Io(io::Error::other(err))),
    };
    if std::fs::read_dir(path)?.next().is_none() {
        removal::remove_empty_dir(&bound).map_err(|err| Error::Io(io::Error::other(err)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "20261022-0805-take-over-5-sessions-w170-memory-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn memory(&self) -> PathBuf {
            files::project_dir(&self.0, Path::new("/repo")).join("memory")
        }
        fn watch(&self) -> Watch {
            Watch::start(&self.0, Path::new("/repo")).unwrap()
        }
        fn remove(&self, artifact: Artifact) {
            let Artifact::ClaudeMemoryDir {
                path,
                identity,
                project_identity,
            } = artifact
            else {
                panic!("not a memory directory")
            };
            remove(&self.0, Path::new(&path), identity, project_identity).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn preexisting_or_unproven_memory_is_never_claimed() {
        let fixture = Fixture::new();
        let mut watch = fixture.watch();
        std::fs::create_dir_all(fixture.memory()).unwrap();
        assert!(watch.artifact().is_some());
        assert!(Watch::start(&fixture.0, Path::new("/repo")).is_none());
        watch.since = SystemTime::now() + Duration::from_secs(60);
        assert!(
            watch.artifact().is_none(),
            "a directory older than this CLI stays"
        );
    }

    #[test]
    fn memory_populated_after_recording_keeps_its_contents_and_project() {
        let fixture = Fixture::new();
        let watch = fixture.watch();
        std::fs::create_dir_all(fixture.memory()).unwrap();
        let artifact = watch.artifact().unwrap();
        std::fs::write(fixture.memory().join("MEMORY.md"), "keep").unwrap();
        fixture.remove(artifact);
        assert_eq!(
            std::fs::read_to_string(fixture.memory().join("MEMORY.md")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn a_replacement_empty_directory_is_not_removed() {
        let fixture = Fixture::new();
        let watch = fixture.watch();
        std::fs::create_dir_all(fixture.memory()).unwrap();
        let artifact = watch.artifact().unwrap();
        std::fs::rename(fixture.memory(), fixture.0.join("original")).unwrap();
        std::fs::create_dir(fixture.memory()).unwrap();
        fixture.remove(artifact);
        assert!(fixture.memory().is_dir());
    }

    #[test]
    fn a_preexisting_project_is_left_after_its_new_memory_goes() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.memory().parent().unwrap()).unwrap();
        let watch = fixture.watch();
        std::fs::create_dir(fixture.memory()).unwrap();
        fixture.remove(watch.artifact().unwrap());
        assert!(!fixture.memory().exists());
        assert!(fixture.memory().parent().unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn memory_replaced_with_a_link_is_not_followed() {
        let fixture = Fixture::new();
        let watch = fixture.watch();
        std::fs::create_dir_all(fixture.memory()).unwrap();
        let artifact = watch.artifact().unwrap();
        let original = fixture.0.join("original");
        std::fs::rename(fixture.memory(), &original).unwrap();
        std::os::unix::fs::symlink(&original, fixture.memory()).unwrap();
        fixture.remove(artifact);
        assert!(original.is_dir());
        assert!(fixture.memory().is_symlink());
    }
}
