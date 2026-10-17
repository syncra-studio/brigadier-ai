//! Pieces shared by macOS and Linux.

use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use nix::sys::signal::{self, Signal};
use nix::unistd::{Pid, Uid};

use crate::{DetachedChild, Error, PrivateFs, Result, SpawnSpec};

pub(crate) struct UnixPrivateFs;

impl PrivateFs for UnixPrivateFs {
    fn create_private_dir(&self, dir: &Path) -> Result<()> {
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent)?;
        }
        match DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => return Ok(()),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err.into()),
        }
        // It already existed: it must be a real directory we own, and nobody else may enter it.
        let meta = fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != Uid::current().as_raw() {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} is not a directory owned by the current user",
                    dir.display()
                ),
            )));
        }
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    fn create_private_file(&self, path: &Path) -> Result<File> {
        Ok(OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?)
    }
}

pub(crate) fn spawn_detached(spec: &SpawnSpec) -> Result<DetachedChild> {
    let mut command = spec.command();
    // SAFETY: `setsid` is async-signal-safe and the closure touches no other state, which is the
    // contract for code running between fork and exec.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(|| nix::unistd::setsid().map(drop).map_err(io::Error::from));
    }
    Ok(DetachedChild::new(command.spawn()?))
}

pub(crate) fn piped_command(spec: &SpawnSpec) -> Command {
    let mut command = spec.piped();
    command.process_group(0);
    if spec.low_priority {
        // SAFETY: `setpriority` is async-signal-safe and the closure touches no other state,
        // which is the contract for code running between fork and exec.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(|| {
                // Lowering one's own priority needs no privilege; a failure leaves it as is.
                libc::setpriority(libc::PRIO_PROCESS, 0, LOW_PRIORITY_NICE);
                Ok(())
            });
        }
    }
    command
}

/// The niceness of low-priority processes.
const LOW_PRIORITY_NICE: libc::c_int = 10;

/// `dir` as [`crate::Processes::in_dir`] may search it: fully resolved (`/tmp` is a link on
/// macOS, a name may differ in case from the folder's own), but never through a link at `dir`
/// itself. A link planted at the folder's own path would aim the search, and the cleanup that
/// ends what it finds, at whatever it points to.
pub(crate) fn search_root(dir: &Path) -> io::Result<std::path::PathBuf> {
    let refused = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} is a link, not a folder", dir.display()),
        )
    };
    if fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(refused());
    }
    let root = dir.canonicalize()?;
    // The root, `.` or a path ending in `..`: no name of its own to plant a link at.
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name()) else {
        return Ok(root);
    };
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    // A link put in its place meanwhile resolves elsewhere: outside the folder's parent, or
    // to another name in it.
    let moved = root.parent() != Some(parent.canonicalize()?.as_path())
        || !root
            .file_name()
            .is_some_and(|real| real.eq_ignore_ascii_case(name));
    if moved || fs::symlink_metadata(dir)?.file_type().is_symlink() {
        return Err(refused());
    }
    Ok(root)
}

pub(crate) fn is_alive(pid: u32) -> bool {
    let Some(pid) = to_pid(pid) else {
        return false;
    };
    match signal::kill(pid, None) {
        Ok(()) => true,
        // It exists but belongs to someone else.
        Err(nix::errno::Errno::EPERM) => true,
        Err(_) => false,
    }
}

pub(crate) fn terminate(pid: u32) -> Result<()> {
    let pid = to_pid(pid).ok_or_else(invalid_pid)?;
    signal::kill(pid, Signal::SIGTERM).map_err(io::Error::from)?;
    Ok(())
}

/// Kills `pid`'s tree. `children` lists a process's children; each process is stopped before
/// its children are listed, so nothing forks past the walk. Every process found is killed, with
/// any process group it leads.
pub(crate) fn kill_tree(pid: u32, children: impl Fn(u32) -> Vec<u32>) -> Result<()> {
    let root = to_pid(pid).ok_or_else(invalid_pid)?;
    let mut tree = vec![pid];
    let mut next = 0;
    while let Some(&parent) = tree.get(next) {
        next += 1;
        if let Some(parent) = to_pid(parent) {
            let _ = signal::kill(parent, Signal::SIGSTOP);
        }
        for child in children(parent) {
            if !tree.contains(&child) {
                tree.push(child);
            }
        }
    }
    for member in tree.into_iter().skip(1).filter_map(to_pid) {
        if nix::unistd::getpgid(Some(member)) == Ok(member) {
            let _ = signal::killpg(member, Signal::SIGKILL);
        }
        let _ = signal::kill(member, Signal::SIGKILL);
    }
    // Piped children lead their own process group, so the group id is their pid.
    match signal::killpg(root, Signal::SIGKILL) {
        Ok(()) => Ok(()),
        Err(nix::errno::Errno::ESRCH) => {
            signal::kill(root, Signal::SIGKILL).map_err(io::Error::from)?;
            Ok(())
        }
        Err(err) => Err(io::Error::from(err).into()),
    }
}

/// Everything below `pid` in the tree `children` describes, parents before children.
pub(crate) fn descendants(pid: u32, children: impl Fn(u32) -> Vec<u32>) -> Vec<u32> {
    let mut tree = vec![pid];
    let mut next = 0;
    while let Some(&parent) = tree.get(next) {
        next += 1;
        for child in children(parent) {
            if !tree.contains(&child) {
                tree.push(child);
            }
        }
    }
    tree.remove(0);
    tree
}

pub(crate) fn suspend(pid: u32) -> Result<()> {
    signal(pid, Signal::SIGSTOP)
}

pub(crate) fn resume(pid: u32) -> Result<()> {
    signal(pid, Signal::SIGCONT)
}

fn signal(pid: u32, which: Signal) -> Result<()> {
    let pid = to_pid(pid).ok_or_else(invalid_pid)?;
    signal::kill(pid, which).map_err(io::Error::from)?;
    Ok(())
}

pub(crate) fn group_of(pid: u32) -> Option<u32> {
    let group = nix::unistd::getpgid(Some(to_pid(pid)?)).ok()?;
    u32::try_from(group.as_raw()).ok()
}

pub(crate) fn kill_group(pid: u32) -> Result<()> {
    let group = to_pid(pid).ok_or_else(invalid_pid)?;
    match signal::killpg(group, Signal::SIGKILL) {
        Ok(()) | Err(nix::errno::Errno::ESRCH) => Ok(()),
        Err(err) => Err(io::Error::from(err).into()),
    }
}

fn to_pid(pid: u32) -> Option<Pid> {
    i32::try_from(pid)
        .ok()
        .filter(|pid| *pid > 0)
        .map(Pid::from_raw)
}

fn invalid_pid() -> Error {
    Error::Io(io::Error::new(io::ErrorKind::InvalidInput, "invalid pid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder, removed when dropped however the test ends.
    struct Temp(std::path::PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_search_root_is_the_real_folder_and_never_a_link_at_its_name() {
        let base =
            std::env::temp_dir().join(format!("brigadier-search-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("Real")).unwrap();
        let base = Temp(base);
        let real = base.0.join("Real").canonicalize().unwrap();
        assert_eq!(search_root(&base.0.join("Real")).unwrap(), real);
        std::os::unix::fs::symlink(base.0.join("Real"), base.0.join("link")).unwrap();
        assert!(search_root(&base.0.join("link")).is_err());
        // A name differing only in case finds the folder where the file system ignores case.
        if base.0.join("real").exists() {
            assert_eq!(search_root(&base.0.join("real")).unwrap(), real);
        }
    }

    #[test]
    fn a_relative_one_name_search_root_resolves() {
        let name = format!("brigadier-search-root-rel-{}", std::process::id());
        fs::create_dir(&name).unwrap();
        let _cleanup = Temp(Path::new(&name).canonicalize().unwrap());
        assert_eq!(
            search_root(Path::new(&name)).unwrap(),
            Path::new(&name).canonicalize().unwrap()
        );
    }
}
