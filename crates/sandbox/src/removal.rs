//! Removing what Brigadier owns, and nothing else.
//!
//! Every removal starts from a [`Bound`] entry: a path strictly inside an allowed root, with no
//! symbolic link anywhere below that root, and the identity (device and inode) the entry had
//! when it was looked at. Removing it checks all of that again, then acts on the very entry
//! that was checked:
//!
//! - [`delete`] (rebuildable data only: scratch, caches, logs, sockets, temp folders) walks
//!   down from the root through directory descriptors opened without following links, checks
//!   the entry's identity through its parent's descriptor and removes the tree below it the
//!   same way, so a folder swapped for a link at any point is never followed.
//! - [`trash`] (anything the user may want back: a Brain, a data directory, folders with user
//!   work, the app) first moves the checked entry, through its parent's descriptor, into a
//!   private quarantine folder of this instance in the same root (the same volume, so it is a
//!   rename), checks that what moved is the entry it expected, and only then asks the system to
//!   move it to the Trash from there. If that fails, it is moved back.
//!
//! Either way success means the entry is really gone. Sizes are allocated sizes (what removing
//! a file gives back), links are never followed and hard links are counted once.

use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Why an entry was not removed (or not bound).
#[derive(Debug, thiserror::Error)]
pub enum RemovalError {
    #[error("{path} is not inside {root}")]
    Outside { path: PathBuf, root: PathBuf },
    #[error("{0} is or goes through a symbolic link")]
    Symlink(PathBuf),
    #[error("{0} changed since it was looked at")]
    Changed(PathBuf),
    #[error("{0} is not there")]
    Gone(PathBuf),
    #[error("{0} is still there")]
    StillThere(PathBuf),
    #[error("refusing {path}: {why}")]
    Refused { path: PathBuf, why: &'static str },
    #[error("moving {path} to the Trash failed: {why}")]
    Trash { path: PathBuf, why: String },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
}

impl RemovalError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, RemovalError>;

/// What an entry was when it was looked at: its device and inode on Unix, its volume serial
/// number and file index on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Identity {
    a: u64,
    b: u64,
}

impl Identity {
    /// Its two numbers, for a durable record ([`Identity::from_parts`] reads them back).
    pub fn parts(self) -> (u64, u64) {
        (self.a, self.b)
    }

    pub fn from_parts((a, b): (u64, u64)) -> Self {
        Self { a, b }
    }
}

/// An entry checked to be strictly inside `root`, reached without any symbolic link below
/// `root`, with the identity it had then.
#[derive(Debug, Clone)]
pub struct Bound {
    root: PathBuf,
    rel: PathBuf,
    path: PathBuf,
    identity: Identity,
    dir: bool,
}

impl Bound {
    /// The entry, as found under the root's resolved path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The root it must stay inside, resolved.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_dir(&self) -> bool {
        self.dir
    }

    pub fn identity(&self) -> Identity {
        self.identity
    }

    /// The device and inode captured by this binding, for durable ownership records.
    #[cfg(unix)]
    pub fn unix_identity(&self) -> (u64, u64) {
        (self.identity.a, self.identity.b)
    }
}

/// Binds `path`, which must be strictly inside `root` (an existing directory; it may itself be
/// reached through links, as `/tmp` is on macOS, and is resolved once here).
pub fn bind(root: &Path, path: &Path) -> Result<Bound> {
    let resolved = std::fs::canonicalize(root).map_err(|err| RemovalError::io(root, err))?;
    let rel = relative(root, &resolved, path)?;
    let path = resolved.join(&rel);
    let (identity, dir) = sys::identify(&resolved, &rel)?;
    Ok(Bound {
        root: resolved,
        rel,
        path,
        identity,
        dir,
    })
}

/// Binds `path` as [`bind`] does, but only while it is still the entry recorded as `identity`
/// when it was first looked at: what a durable record names is bound again, never whatever
/// took its place since.
pub fn bind_as(root: &Path, path: &Path, identity: Identity) -> Result<Bound> {
    let bound = bind(root, path)?;
    if bound.identity == identity {
        Ok(bound)
    } else {
        Err(RemovalError::Changed(bound.path))
    }
}

/// Checks that `bound` is still what it was: inside its root, reached without links, the
/// same entry.
pub fn recheck(bound: &Bound) -> Result<()> {
    let (identity, _) = sys::identify(&bound.root, &bound.rel)?;
    if identity == bound.identity {
        Ok(())
    } else {
        Err(RemovalError::Changed(bound.path.clone()))
    }
}

/// Deletes `bound` for good (with everything in it), after checking it again. Only for data
/// Brigadier can rebuild or never needs again.
pub fn delete(bound: &Bound) -> Result<()> {
    sys::delete(bound)
}

/// Removes `bound`, a folder, only while it is empty, after checking it again. What something
/// put in it meanwhile keeps it.
pub fn remove_empty_dir(bound: &Bound) -> Result<()> {
    recheck(bound)?;
    std::fs::remove_dir(&bound.path).map_err(|err| RemovalError::io(&bound.path, err))?;
    if is_gone(&bound.path) {
        Ok(())
    } else {
        Err(RemovalError::StillThere(bound.path.clone()))
    }
}

/// Moves `bound` to the Trash, after checking it again, through a quarantine folder named for
/// `owner` (this instance's id) in its root.
pub fn trash(bound: &Bound, owner: &str) -> Result<()> {
    if owner.is_empty() || !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(RemovalError::Refused {
            path: bound.path.clone(),
            why: "the quarantine owner is not a plain name",
        });
    }
    sys::trash(bound, &format!(".brigadier-trash-{owner}"))
}

/// Whether nothing is at `path` any more (a link counts as something).
pub fn is_gone(path: &Path) -> bool {
    matches!(std::fs::symlink_metadata(path), Err(err) if err.kind() == io::ErrorKind::NotFound)
}

/// The disk space `path` takes (allocated, not apparent), without following links and counting
/// hard-linked files once. Unreadable parts count as nothing.
pub fn allocated_size(path: &Path) -> u64 {
    let mut seen = std::collections::HashSet::new();
    let mut total = 0;
    let mut stack = vec![path.to_owned()];
    while let Some(next) = stack.pop() {
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir()
            && let Ok(entries) = std::fs::read_dir(&next)
        {
            stack.extend(entries.flatten().map(|entry| entry.path()));
        }
        total += sys::allocated(&meta, &mut seen);
    }
    total
}

/// Refuses a data directory that could not be Brigadier's: the file-system root, the home
/// folder, or a folder without the event store and the run folder a daemon creates.
pub fn check_data_dir(dir: &Path) -> Result<()> {
    let refused = |why| {
        Err(RemovalError::Refused {
            path: dir.to_owned(),
            why,
        })
    };
    let resolved = std::fs::canonicalize(dir).map_err(|err| RemovalError::io(dir, err))?;
    if resolved.parent().is_none() {
        return refused("it is the root of the file system");
    }
    if dirs::home_dir()
        .and_then(|home| std::fs::canonicalize(home).ok())
        .is_some_and(|home| home == resolved || home.starts_with(&resolved))
    {
        return refused("it is the home folder or holds it");
    }
    let is = |name: &str, dir: bool| {
        std::fs::symlink_metadata(resolved.join(name)).is_ok_and(|meta| {
            !meta.file_type().is_symlink() && if dir { meta.is_dir() } else { meta.is_file() }
        })
    };
    if !is("brigadier.db", false) || !is("run", true) {
        return refused("it has no brigadier.db and run folder, so it is not a data directory");
    }
    Ok(())
}

/// What a folder's [`crate::OWNER_MARKER`] says, read without following links.
pub fn read_owner_marker(dir: &Path) -> Option<String> {
    sys::read_small_file(&dir.join(crate::OWNER_MARKER))
}

/// `path` relative to `root` (given as is or resolved): normal components only, not empty.
fn relative(root: &Path, resolved: &Path, path: &Path) -> Result<PathBuf> {
    let outside = || RemovalError::Outside {
        path: path.to_owned(),
        root: root.to_owned(),
    };
    let rel = path
        .strip_prefix(resolved)
        .or_else(|_| path.strip_prefix(root))
        .map_err(|_| outside())?;
    if rel.as_os_str().is_empty() {
        return Err(RemovalError::Refused {
            path: path.to_owned(),
            why: "it is the root itself",
        });
    }
    if !rel
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(outside());
    }
    Ok(rel.to_owned())
}

/// The components of a checked relative path: every folder on the way, then the entry's name.
fn split(rel: &Path) -> (Vec<OsString>, OsString) {
    let mut parts: Vec<OsString> = rel
        .components()
        .map(|component| component.as_os_str().to_owned())
        .collect();
    let last = parts.pop().unwrap_or_default();
    (parts, last)
}

#[cfg(unix)]
mod sys {
    use std::collections::HashSet;
    use std::ffi::{OsStr, OsString};
    use std::io::{self, Read as _};
    use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
    use std::path::Path;

    use nix::dir::Dir;
    use nix::errno::Errno;
    use nix::fcntl::{AtFlags, OFlag, open, openat, renameat};
    use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat, mkdirat};
    use nix::unistd::{UnlinkatFlags, unlinkat};

    use super::{Bound, Identity, RemovalError, Result, split};

    const DIR_FLAGS: OFlag = OFlag::O_RDONLY
        .union(OFlag::O_DIRECTORY)
        .union(OFlag::O_NOFOLLOW)
        .union(OFlag::O_CLOEXEC);

    // `dev_t` and `ino_t` differ in width and sign between platforms.
    #[allow(clippy::unnecessary_cast)]
    fn identity(stat: &FileStat) -> Identity {
        Identity {
            a: stat.st_dev as u64,
            b: stat.st_ino as u64,
        }
    }

    fn kind(stat: &FileStat) -> SFlag {
        SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT
    }

    fn io_err(path: &Path, errno: Errno) -> RemovalError {
        match errno {
            Errno::ELOOP | Errno::ENOTDIR => RemovalError::Symlink(path.to_owned()),
            Errno::ENOENT => RemovalError::Gone(path.to_owned()),
            other => RemovalError::io(path, io::Error::from(other)),
        }
    }

    /// Opens every folder from `root` down to the entry's parent without following links.
    fn open_parent(root: &Path, rel: &Path) -> Result<(OwnedFd, OsString)> {
        let (folders, last) = split(rel);
        let mut fd = open(root, DIR_FLAGS, Mode::empty()).map_err(|err| io_err(root, err))?;
        let mut at = root.to_owned();
        for folder in folders {
            at.push(&folder);
            fd = openat(&fd, folder.as_os_str(), DIR_FLAGS, Mode::empty())
                .map_err(|err| io_err(&at, err))?;
        }
        Ok((fd, last))
    }

    /// The entry's stat through its parent's descriptor; a link is refused.
    fn stat_at(parent: impl AsFd, name: &OsStr, path: &Path) -> Result<FileStat> {
        let stat =
            fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(|err| io_err(path, err))?;
        if kind(&stat) == SFlag::S_IFLNK {
            return Err(RemovalError::Symlink(path.to_owned()));
        }
        Ok(stat)
    }

    pub fn identify(root: &Path, rel: &Path) -> Result<(Identity, bool)> {
        let path = root.join(rel);
        let (parent, last) = open_parent(root, rel)?;
        let stat = stat_at(&parent, &last, &path)?;
        Ok((identity(&stat), kind(&stat) == SFlag::S_IFDIR))
    }

    /// The parent's descriptor and the entry's name, once the entry is checked to be `bound`.
    fn reach(bound: &Bound) -> Result<(OwnedFd, OsString, FileStat)> {
        let (parent, last) = open_parent(&bound.root, &bound.rel)?;
        let stat = stat_at(&parent, &last, &bound.path)?;
        if identity(&stat) != bound.identity {
            return Err(RemovalError::Changed(bound.path.clone()));
        }
        Ok((parent, last, stat))
    }

    fn gone_at(parent: impl AsFd, name: &OsStr) -> bool {
        matches!(
            fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW),
            Err(Errno::ENOENT)
        )
    }

    pub fn delete(bound: &Bound) -> Result<()> {
        let (parent, last, stat) = reach(bound)?;
        remove_at(parent.as_fd(), &last, &stat, &bound.path)?;
        if gone_at(&parent, &last) {
            Ok(())
        } else {
            Err(RemovalError::StillThere(bound.path.clone()))
        }
    }

    /// Removes the entry `name` in `parent` (which `stat` describes) and, for a folder,
    /// everything in it, never following a link.
    fn remove_at(parent: BorrowedFd<'_>, name: &OsStr, stat: &FileStat, path: &Path) -> Result<()> {
        if kind(stat) != SFlag::S_IFDIR {
            return unlinkat(parent, name, UnlinkatFlags::NoRemoveDir)
                .map_err(|err| RemovalError::io(path, io::Error::from(err)));
        }
        let fd = openat(parent, name, DIR_FLAGS, Mode::empty()).map_err(|err| io_err(path, err))?;
        // The folder opened must be the one looked at, not one put in its place meanwhile.
        let opened = fstat(&fd).map_err(|err| io_err(path, err))?;
        if identity(&opened) != identity(stat) {
            return Err(RemovalError::Changed(path.to_owned()));
        }
        let mut dir = Dir::from_fd(fd).map_err(|err| io_err(path, err))?;
        let names: Vec<OsString> = dir
            .iter()
            .flatten()
            .map(|entry| OsStr::from_bytes(entry.file_name().to_bytes()).to_owned())
            .filter(|name| name != "." && name != "..")
            .collect();
        for child in names {
            let child_path = path.join(&child);
            let child_stat = match fstatat(&dir, child.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW) {
                Ok(stat) => stat,
                Err(Errno::ENOENT) => continue,
                Err(err) => return Err(io_err(&child_path, err)),
            };
            remove_at(dir.as_fd(), &child, &child_stat, &child_path)?;
        }
        drop(dir);
        unlinkat(parent, name, UnlinkatFlags::RemoveDir)
            .map_err(|err| RemovalError::io(path, io::Error::from(err)))
    }

    pub fn trash(bound: &Bound, quarantine: &str) -> Result<()> {
        let (parent, last, _) = reach(bound)?;
        let root = open(bound.root.as_path(), DIR_FLAGS, Mode::empty())
            .map_err(|err| io_err(&bound.root, err))?;
        let held = bound.root.join(quarantine);
        match mkdirat(&root, quarantine, Mode::S_IRWXU) {
            Ok(()) | Err(Errno::EEXIST) => {}
            Err(err) => return Err(io_err(&held, err)),
        }
        let hold = openat(&root, quarantine, DIR_FLAGS, Mode::empty())
            .map_err(|err| io_err(&held, err))?;
        let hold_stat = fstat(&hold).map_err(|err| io_err(&held, err))?;
        if hold_stat.st_uid != nix::unistd::getuid().as_raw() || hold_stat.st_mode & 0o077 != 0 {
            return Err(RemovalError::Refused {
                path: held,
                why: "the quarantine folder is not private to this user",
            });
        }
        // A name free in the quarantine (only this user can write there).
        let mut name = last.clone();
        let mut n = 2;
        while !gone_at(&hold, &name) {
            name = OsString::from(format!("{} {n}", last.to_string_lossy()));
            n += 1;
        }
        renameat(&parent, last.as_os_str(), &hold, name.as_os_str())
            .map_err(|err| io_err(&bound.path, err))?;
        let moved = stat_at(&hold, &name, &held.join(&name));
        if !moved.is_ok_and(|stat| identity(&stat) == bound.identity) {
            // Something else was put there between the check and the move: put it back.
            let _ = renameat(&hold, name.as_os_str(), &parent, last.as_os_str());
            let _ = unlinkat(&root, quarantine, UnlinkatFlags::RemoveDir);
            return Err(RemovalError::Changed(bound.path.clone()));
        }
        let staged = held.join(&name);
        let outcome = crate::trashcan::to_trash(&staged);
        let result = match outcome {
            Ok(()) if gone_at(&hold, &name) => Ok(()),
            Ok(()) => Err(RemovalError::StillThere(bound.path.clone())),
            Err(why) => Err(RemovalError::Trash {
                path: bound.path.clone(),
                why,
            }),
        };
        if result.is_err() && !gone_at(&hold, &name) {
            let _ = renameat(&hold, name.as_os_str(), &parent, last.as_os_str());
        }
        let _ = unlinkat(&root, quarantine, UnlinkatFlags::RemoveDir);
        result
    }

    pub fn allocated(meta: &std::fs::Metadata, seen: &mut HashSet<(u64, u64)>) -> u64 {
        if meta.nlink() > 1 && !meta.is_dir() && !seen.insert((meta.dev(), meta.ino())) {
            return 0;
        }
        meta.blocks().saturating_mul(512)
    }

    pub fn read_small_file(path: &Path) -> Option<String> {
        let mut text = String::new();
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .ok()?
            .take(4096)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }
}

#[cfg(windows)]
mod sys {
    use std::collections::HashSet;
    use std::io::Read as _;
    use std::os::windows::fs::MetadataExt as _;
    use std::path::Path;

    use super::{Bound, Identity, RemovalError, Result, split};

    /// Walks from `root` to the entry checking that nothing on the way is a link or junction.
    fn walk(root: &Path, rel: &Path) -> Result<std::fs::Metadata> {
        let (folders, last) = split(rel);
        let mut at = root.to_owned();
        for folder in folders.iter().chain(std::iter::once(&last)) {
            at.push(folder);
            let meta = std::fs::symlink_metadata(&at).map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    RemovalError::Gone(at.clone())
                } else {
                    RemovalError::io(&at, err)
                }
            })?;
            // FILE_ATTRIBUTE_REPARSE_POINT: links and junctions.
            if meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0 {
                return Err(RemovalError::Symlink(at));
            }
        }
        std::fs::symlink_metadata(&at).map_err(|err| RemovalError::io(&at, err))
    }

    /// The entry's volume serial number and file index, read through a handle to the entry
    /// itself (a link or junction is opened, not followed).
    #[allow(unsafe_code)]
    fn identity(path: &Path) -> Result<Identity> {
        use std::os::windows::fs::OpenOptionsExt as _;
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(0)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
            .map_err(|err| RemovalError::io(path, err))?;
        // SAFETY: all-zero is a valid BY_HANDLE_FILE_INFORMATION (plain integers).
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: the handle stays open for the call and `info` is a valid place to write.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(RemovalError::io(path, std::io::Error::last_os_error()));
        }
        Ok(Identity {
            a: u64::from(info.dwVolumeSerialNumber),
            b: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        })
    }

    pub fn identify(root: &Path, rel: &Path) -> Result<(Identity, bool)> {
        let meta = walk(root, rel)?;
        Ok((identity(&root.join(rel))?, meta.is_dir()))
    }

    fn checked(bound: &Bound) -> Result<()> {
        walk(&bound.root, &bound.rel)?;
        if identity(&bound.path)? != bound.identity {
            return Err(RemovalError::Changed(bound.path.clone()));
        }
        Ok(())
    }

    pub fn delete(bound: &Bound) -> Result<()> {
        checked(bound)?;
        let removed = if bound.dir {
            std::fs::remove_dir_all(&bound.path)
        } else {
            std::fs::remove_file(&bound.path)
        };
        removed.map_err(|err| RemovalError::io(&bound.path, err))?;
        if super::is_gone(&bound.path) {
            Ok(())
        } else {
            Err(RemovalError::StillThere(bound.path.clone()))
        }
    }

    pub fn trash(bound: &Bound, _quarantine: &str) -> Result<()> {
        checked(bound)?;
        crate::trashcan::to_trash(&bound.path).map_err(|why| RemovalError::Trash {
            path: bound.path.clone(),
            why,
        })?;
        if super::is_gone(&bound.path) {
            Ok(())
        } else {
            Err(RemovalError::StillThere(bound.path.clone()))
        }
    }

    pub fn allocated(meta: &std::fs::Metadata, _seen: &mut HashSet<(u64, u64)>) -> u64 {
        if meta.is_dir() { 0 } else { meta.file_size() }
    }

    pub fn read_small_file(path: &Path) -> Option<String> {
        let meta = std::fs::symlink_metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let mut text = String::new();
        std::fs::File::open(path)
            .ok()?
            .take(4096)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }
}
