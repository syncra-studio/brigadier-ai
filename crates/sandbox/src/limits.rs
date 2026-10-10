//! The open-files limit. launchd starts the daemon with a soft limit of 256, and every CLI,
//! build and test inherits it from there: too few for a test suite or a bundler. The daemon
//! raises its own soft limit once at start ([`raise_open_files`]), so everything it starts
//! inherits the higher one.

/// A limit that isn't limited.
pub const UNLIMITED: u64 = u64::MAX;

/// The soft limit the daemon asks for, when the hard limit allows it.
pub const OPEN_FILES_TARGET: u64 = 65_536;

/// A process's open-files limits; [`UNLIMITED`] for none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenFiles {
    pub soft: u64,
    pub hard: u64,
}

/// This process's open-files limits, or `None` where they can't be read (and on Windows,
/// which has none of this kind).
#[must_use]
pub fn open_files() -> Option<OpenFiles> {
    #[cfg(unix)]
    {
        use nix::sys::resource::{Resource, getrlimit};
        let (soft, hard) = getrlimit(Resource::RLIMIT_NOFILE).ok()?;
        Some(OpenFiles {
            soft: from_rlim(soft),
            hard: from_rlim(hard),
        })
    }
    #[cfg(not(unix))]
    None
}

/// Raises this process's soft open-files limit to `min(hard, OPEN_FILES_TARGET)`, never
/// lowering it; the hard limit stays. macOS refuses a soft limit above its per-process
/// maximum (`kern.maxfilesperproc`), so a refused raise tries `OPEN_MAX` (10240) instead.
/// Returns the limits it ends with.
pub fn raise_open_files() -> std::io::Result<OpenFiles> {
    #[cfg(unix)]
    {
        use nix::sys::resource::{Resource, setrlimit};
        let now = open_files().ok_or_else(|| std::io::Error::other("unreadable limits"))?;
        for target in [OPEN_FILES_TARGET, 10_240] {
            let soft = target.min(now.hard);
            if soft <= now.soft {
                return Ok(now);
            }
            if setrlimit(Resource::RLIMIT_NOFILE, to_rlim(soft), to_rlim(now.hard)).is_ok() {
                return open_files().ok_or_else(|| std::io::Error::other("unreadable limits"));
            }
        }
        Err(std::io::Error::other(format!(
            "the system refused to raise the open-files limit from {}",
            now.soft
        )))
    }
    #[cfg(not(unix))]
    Ok(OpenFiles {
        soft: UNLIMITED,
        hard: UNLIMITED,
    })
}

#[cfg(unix)]
fn from_rlim(value: nix::sys::resource::rlim_t) -> u64 {
    if value == nix::libc::RLIM_INFINITY {
        UNLIMITED
    } else {
        value
    }
}

#[cfg(unix)]
fn to_rlim(value: u64) -> nix::sys::resource::rlim_t {
    if value == UNLIMITED {
        nix::libc::RLIM_INFINITY
    } else {
        value
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// What the daemon starts inherits the raised limit: a shell it spawns reports it.
    #[test]
    fn a_child_inherits_the_raised_open_files_limit() {
        let raised = raise_open_files().unwrap();
        assert!(raised.soft >= 8192, "{raised:?}");
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", "ulimit -Sn"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        let soft: u64 = if text.trim() == "unlimited" {
            UNLIMITED
        } else {
            text.trim().parse().unwrap()
        };
        assert_eq!(soft, raised.soft);
        assert!(soft >= 8192);
    }
}
