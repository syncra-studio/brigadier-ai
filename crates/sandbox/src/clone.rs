//! Copy-on-write copies of folders, for warming a task's worktree with the build caches of the
//! user's checkout.
//!
//! - macOS: `clonefile(2)` clones the whole tree on APFS. Nothing is written until either side
//!   changes a file, and then only that file diverges.
//! - Linux: `cp -a --reflink=always`, which fails rather than falling back to a full copy on file
//!   systems that can't share blocks. It is killed when it runs past its deadline.
//! - Elsewhere: nothing is copied ([`SUPPORTED`] is false). There is no way there to publish a
//!   copy without possibly replacing what is already in its place.
//!
//! All of it happens in a [`Folder`]: one reached from a root without following any link, and
//! held open, so a link swapped in above it can't send the copy (or its publishing) elsewhere.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

/// Whether this platform can copy and publish folders at all.
pub const SUPPORTED: bool = cfg!(any(target_os = "macos", target_os = "linux"));

/// A folder inside a root, every folder from the root down to it a real one (not a link).
#[derive(Debug)]
pub struct Folder {
    path: PathBuf,
    #[cfg(unix)]
    fd: std::os::fd::OwnedFd,
}

impl Folder {
    /// Opens `root/rel`. `rel` must be plain folder names (no `..`, nothing absolute), and each
    /// must be a real folder; with `create`, missing ones are made. `root` itself is trusted.
    pub fn open(root: &Path, rel: &Path, create: bool) -> io::Result<Self> {
        let mut names = Vec::new();
        for part in rel.components() {
            match part {
                Component::Normal(name) => names.push(name),
                Component::CurDir => {}
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("{} leaves {}", rel.display(), root.display()),
                    ));
                }
            }
        }
        open_folder(root, &names, create)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Copies `name` in `src` into this folder as `new`, which must not exist yet. Links are
    /// copied as links, never followed. Fails once `deadline` passes. On failure `new` may be
    /// partly written: the caller removes it.
    pub fn clone_in(
        &self,
        src: &Folder,
        name: &OsStr,
        new: &OsStr,
        deadline: Instant,
    ) -> io::Result<()> {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "out of time"));
        }
        platform_clone(src, name, self, new, deadline)
    }

    /// Renames `from` to `to` in this folder, failing with `AlreadyExists` when `to` is there
    /// (it is never replaced, even by an empty folder).
    pub fn rename_new(&self, from: &OsStr, to: &OsStr) -> io::Result<()> {
        platform_rename_new(self, from, to)
    }
}

#[cfg(unix)]
fn open_folder(root: &Path, names: &[&OsStr], create: bool) -> io::Result<Folder> {
    use nix::fcntl::{OFlag, open, openat};
    use nix::sys::stat::{Mode, mkdirat};
    let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let mut fd = open(root, flags, Mode::empty()).map_err(io::Error::from)?;
    let mut path = root.to_owned();
    for name in names {
        path.push(name);
        let next = match openat(&fd, *name, flags, Mode::empty()) {
            Err(nix::errno::Errno::ENOENT) if create => {
                match mkdirat(&fd, *name, Mode::from_bits_truncate(0o777)) {
                    Ok(()) | Err(nix::errno::Errno::EEXIST) => {}
                    Err(err) => return Err(err.into()),
                }
                openat(&fd, *name, flags, Mode::empty())
            }
            other => other,
        };
        fd = next.map_err(|err| match err {
            // A link where a folder should be.
            nix::errno::Errno::ELOOP | nix::errno::Errno::ENOTDIR => {
                io::Error::other(format!("{} is a link or not a folder", path.display()))
            }
            err => err.into(),
        })?;
    }
    Ok(Folder { path, fd })
}

#[cfg(not(unix))]
fn open_folder(root: &Path, names: &[&OsStr], create: bool) -> io::Result<Folder> {
    let mut path = root.to_owned();
    for name in names {
        path.push(name);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() && !meta.is_symlink() => {}
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "{} is a link or not a folder",
                    path.display()
                )));
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound && create => {
                std::fs::create_dir(&path)?;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(Folder { path })
}

#[cfg(target_os = "macos")]
fn platform_clone(
    src: &Folder,
    name: &OsStr,
    dst: &Folder,
    new: &OsStr,
    _deadline: Instant,
) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    // `CLONE_NOFOLLOW` from <sys/clonefile.h>: clone a link itself, not what it points at.
    const CLONE_NOFOLLOW: u32 = 0x0001;
    let from = CString::new(name.as_bytes())?;
    let to = CString::new(new.as_bytes())?;
    // One call that can't be interrupted, but it only copies the tree's metadata.
    // SAFETY: both descriptors are open and both names are NUL-terminated for the whole call.
    #[allow(unsafe_code)]
    let status = unsafe {
        libc::clonefileat(
            src.fd.as_raw_fd(),
            from.as_ptr(),
            dst.fd.as_raw_fd(),
            to.as_ptr(),
            CLONE_NOFOLLOW,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// The most of `cp`'s error output kept.
#[cfg(target_os = "linux")]
const MAX_STDERR: usize = 4 << 10;

#[cfg(target_os = "linux")]
fn platform_clone(
    src: &Folder,
    name: &OsStr,
    dst: &Folder,
    new: &OsStr,
    deadline: Instant,
) -> io::Result<()> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let into = dst.fd.as_raw_fd();
    let mut command = Command::new("cp");
    command
        .args(["-a", "--reflink=always", "--no-target-directory", "--"])
        .arg(src.path().join(name))
        .arg(new)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    // `cp` starts in the destination folder itself and writes `new` relative to it, so a
    // folder above it swapped for a link can't redirect the copy.
    // SAFETY: `fchdir` is async-signal-safe, and `into` stays open until the child has exited.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            if libc::fchdir(into) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
    let mut child = command.spawn()?;
    let stderr = child.stderr.take();
    // Read on the side (a full pipe would stall `cp`), keeping only the start.
    let reader = std::thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 4096];
        let Some(mut pipe) = stderr else {
            return kept;
        };
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let room = MAX_STDERR.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
        }
        kept
    });
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    let stderr = reader.join().unwrap_or_default();
    match status {
        None => Err(io::Error::new(io::ErrorKind::TimedOut, "out of time")),
        Some(status) if status.success() => Ok(()),
        Some(_) => Err(io::Error::other(format!(
            "cp --reflink=always failed: {}",
            String::from_utf8_lossy(&stderr).trim()
        ))),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_clone(
    _src: &Folder,
    _name: &OsStr,
    _dst: &Folder,
    _new: &OsStr,
    _deadline: Instant,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "copying folders isn't supported here",
    ))
}

#[cfg(target_os = "macos")]
fn platform_rename_new(folder: &Folder, from: &OsStr, to: &OsStr) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let source = CString::new(from.as_bytes())?;
    let target = CString::new(to.as_bytes())?;
    let fd = folder.fd.as_raw_fd();
    // SAFETY: the descriptor is open and both names are NUL-terminated for the whole call.
    #[allow(unsafe_code)]
    let status =
        unsafe { libc::renameatx_np(fd, source.as_ptr(), fd, target.as_ptr(), libc::RENAME_EXCL) };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// `renameat2(2)` called directly, on every C library (musl has no wrapper for it).
#[cfg(target_os = "linux")]
fn platform_rename_new(folder: &Folder, from: &OsStr, to: &OsStr) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    // `RENAME_NOREPLACE` from <linux/fs.h>.
    const RENAME_NOREPLACE: libc::c_uint = 1;
    let source = CString::new(from.as_bytes())?;
    let target = CString::new(to.as_bytes())?;
    let fd = folder.fd.as_raw_fd();
    // SAFETY: the descriptor is open and both names are NUL-terminated for the whole call.
    #[allow(unsafe_code)]
    let status = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            fd,
            source.as_ptr(),
            fd,
            target.as_ptr(),
            RENAME_NOREPLACE,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// No rename here refuses to replace what is in its place, so nothing is published.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_rename_new(_folder: &Folder, _from: &OsStr, _to: &OsStr) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "renaming without replacing isn't supported here",
    ))
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn temp(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-clone-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Temp(std::fs::canonicalize(dir).unwrap())
    }

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    #[test]
    fn clones_a_tree_and_refuses_an_existing_destination() {
        let root = temp("tree");
        std::fs::create_dir_all(root.join("src/a/b")).unwrap();
        std::fs::write(root.join("src/a/b/file"), b"hello").unwrap();
        let folder = Folder::open(&root, Path::new(""), false).unwrap();
        // Tests run on whatever file system the temp folder is on; one that can't share
        // blocks fails cleanly, which is all warming needs.
        match folder.clone_in(&folder, OsStr::new("src"), OsStr::new("dst"), later()) {
            Ok(()) => {
                let dst = root.join("dst");
                assert_eq!(std::fs::read(dst.join("a/b/file")).unwrap(), b"hello");
                std::fs::write(dst.join("a/b/file"), b"changed").unwrap();
                assert_eq!(std::fs::read(root.join("src/a/b/file")).unwrap(), b"hello");
            }
            Err(_) => assert!(!root.join("dst/a/b/file").exists()),
        }
        std::fs::create_dir_all(root.join("there")).unwrap();
        assert!(
            folder
                .clone_in(&folder, OsStr::new("src"), OsStr::new("there"), later())
                .is_err()
        );
        assert!(!root.join("there/a").exists());
        // Past its deadline nothing is copied.
        let past = Instant::now();
        let err = folder
            .clone_in(&folder, OsStr::new("src"), OsStr::new("late"), past)
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(!root.join("late").exists());
    }

    #[test]
    fn rename_new_never_replaces_even_an_empty_folder() {
        let root = temp("rename");
        std::fs::create_dir_all(root.join("from/inner")).unwrap();
        std::fs::create_dir_all(root.join("to")).unwrap();
        let folder = Folder::open(&root, Path::new(""), false).unwrap();
        let err = folder
            .rename_new(OsStr::new("from"), OsStr::new("to"))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert!(root.join("from/inner").is_dir());
        folder
            .rename_new(OsStr::new("from"), OsStr::new("fresh"))
            .unwrap();
        assert!(root.join("fresh/inner").is_dir());
    }

    #[test]
    fn folders_are_reached_without_following_links() {
        let root = temp("folders");
        let outside = temp("outside");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::os::unix::fs::symlink(&*outside, root.join("a/link")).unwrap();
        assert_eq!(
            Folder::open(&root, Path::new("a/b"), false).unwrap().path(),
            root.join("a/b")
        );
        // A link anywhere on the way is refused, even when creating.
        assert!(Folder::open(&root, Path::new("a/link"), false).is_err());
        assert!(Folder::open(&root, Path::new("a/link/new"), true).is_err());
        assert!(!outside.join("new").exists());
        assert!(Folder::open(&root, Path::new("a/../a"), false).is_err());
        assert!(Folder::open(&root, Path::new("/tmp"), false).is_err());
        // Missing folders are made one by one.
        Folder::open(&root, Path::new("a/c/d"), true).unwrap();
        assert!(root.join("a/c/d").is_dir());
        assert!(Folder::open(&root, Path::new("a/e"), false).is_err());
    }
}
