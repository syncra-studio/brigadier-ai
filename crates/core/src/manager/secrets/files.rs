//! Regular-file copies within checked roots. Directory handles keep a link swapped into a
//! parent from redirecting an open or a write; publication replaces a name, never its referent.

use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Read, Seek};
use std::path::{Component, Path, PathBuf};

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "keep the path inside the folder, without links",
    )
}

struct Folder {
    path: PathBuf,
    handle: File,
    // Windows refuses deletion/renaming while these non-share-delete handles are open.
    #[cfg(windows)]
    parents: Vec<File>,
}

impl Folder {
    fn root(root: &Path) -> io::Result<Self> {
        if !fs::symlink_metadata(root)?.is_dir() {
            return Err(invalid());
        }
        let path = root.canonicalize()?;
        #[cfg(unix)]
        let handle = {
            use nix::fcntl::open;
            use nix::sys::stat::Mode;
            File::from(open(&path, dir_flags(), Mode::empty())?)
        };
        #[cfg(windows)]
        let handle = open_directory(&path)?;
        Ok(Self {
            path,
            handle,
            #[cfg(windows)]
            parents: Vec::new(),
        })
    }

    fn parent(root: &Path, file: &Path, create: bool) -> io::Result<Self> {
        if file.as_os_str().is_empty()
            || file
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(invalid());
        }
        let mut folder = Self::root(root)?;
        let root = folder.path.clone();
        for part in file.parent().ok_or_else(invalid)?.components() {
            let Component::Normal(part) = part else {
                return Err(invalid());
            };
            let path = folder.path.join(part);
            #[cfg(unix)]
            let handle = {
                use nix::errno::Errno;
                use nix::fcntl::openat;
                use nix::sys::stat::{Mode, mkdirat};
                if create {
                    match mkdirat(&folder.handle, part, Mode::S_IRWXU) {
                        Ok(()) | Err(Errno::EEXIST) => {}
                        Err(err) => return Err(err.into()),
                    }
                }
                File::from(openat(&folder.handle, part, dir_flags(), Mode::empty())?)
            };
            #[cfg(windows)]
            let handle = {
                if create {
                    match fs::create_dir(&path) {
                        Ok(()) => {}
                        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(err) => return Err(err),
                    }
                }
                open_directory(&path)?
            };
            #[cfg(windows)]
            folder.parents.push(folder.handle);
            folder.handle = handle;
            folder.path = path;
            folder.check_inside(&root)?;
        }
        folder.check_inside(&root)?;
        Ok(folder)
    }

    fn check_inside(&self, root: &Path) -> io::Result<()> {
        if !fs::symlink_metadata(&self.path)?.is_dir()
            || !self.path.canonicalize()?.starts_with(root)
        {
            return Err(invalid());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let named = fs::metadata(&self.path)?;
            let opened = self.handle.metadata()?;
            if (named.dev(), named.ino()) != (opened.dev(), opened.ino()) {
                return Err(invalid());
            }
        }
        Ok(())
    }

    fn regular(&self, name: &OsStr) -> io::Result<File> {
        let path = self.path.join(name);
        if !fs::symlink_metadata(&path)?.is_file() {
            return Err(invalid());
        }
        #[cfg(unix)]
        let file = {
            use nix::fcntl::{OFlag, openat};
            use nix::sys::stat::Mode;
            File::from(openat(
                &self.handle,
                name,
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )?)
        };
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt as _;
            fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .custom_flags(0x00200000)
                .open(path)? // FILE_FLAG_OPEN_REPARSE_POINT
        };
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(invalid());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt as _;
            if meta.file_attributes() & 0x400 != 0 {
                // FILE_ATTRIBUTE_REPARSE_POINT
                return Err(invalid());
            }
        }
        Ok(file)
    }
}

#[cfg(unix)]
fn dir_flags() -> nix::fcntl::OFlag {
    use nix::fcntl::OFlag;
    OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC
}

#[cfg(windows)]
fn open_directory(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
    // FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT; no FILE_SHARE_DELETE.
    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .custom_flags(0x02200000)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_dir() || meta.file_attributes() & 0x400 != 0 {
        return Err(invalid());
    }
    Ok(file)
}

/// Source links (including linked parents) and special files are skipped before reading.
pub(super) fn source(root: &Path, file: &Path) -> Option<File> {
    match Folder::parent(root, file, false)
        .and_then(|folder| folder.regular(file.file_name().ok_or_else(invalid)?))
    {
        Ok(file) => Some(file),
        Err(_) => {
            tracing::info!(file = %file.display(), "copy source is not a regular file without links; skipped");
            None
        }
    }
}

pub(super) fn text(root: &Path, file: &Path) -> Option<String> {
    let mut text = String::new();
    source(root, file)?.read_to_string(&mut text).ok()?;
    Some(text)
}

/// Publish a fresh file in its checked parent; existing links and non-files are refused.
pub(super) fn copy(source: &mut File, root: &Path, file: &Path) -> io::Result<()> {
    let canonical_root = root.canonicalize()?;
    let folder = Folder::parent(root, file, true)?;
    let name = file.file_name().ok_or_else(invalid)?;
    let target = folder.path.join(name);
    match fs::symlink_metadata(&target) {
        Ok(meta) => {
            if !meta.is_file() || !target.canonicalize()?.starts_with(&canonical_root) {
                return Err(invalid());
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let temp = format!(".brigadier-copy-{}.part", uuid::Uuid::new_v4().simple());
    #[cfg(unix)]
    let mut output = {
        use nix::fcntl::{OFlag, openat};
        use nix::sys::stat::Mode;
        File::from(openat(
            &folder.handle,
            temp.as_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?)
    };
    #[cfg(windows)]
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.path.join(&temp))?;
    let placed = (|| {
        io::copy(source, &mut output)?;
        output.set_permissions(source.metadata()?.permissions())?;
        output.sync_all()?;
        drop(output);
        folder.check_inside(&canonical_root)?;
        #[cfg(unix)]
        nix::fcntl::renameat(&folder.handle, temp.as_str(), &folder.handle, name)?;
        #[cfg(windows)]
        fs::rename(folder.path.join(&temp), &target)?;
        Ok(())
    })();
    if placed.is_err() {
        #[cfg(unix)]
        let _ = nix::unistd::unlinkat(
            &folder.handle,
            temp.as_str(),
            nix::unistd::UnlinkatFlags::NoRemoveDir,
        );
        #[cfg(windows)]
        let _ = fs::remove_file(folder.path.join(&temp));
    }
    placed
}

/// Read redaction values from the same open source that was copied.
pub(super) fn copied_text(source: &mut File) -> io::Result<String> {
    source.rewind()?;
    let mut text = String::new();
    source.read_to_string(&mut text)?;
    Ok(text)
}
