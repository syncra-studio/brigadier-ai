//! The one directory the helper keeps its state in.
//!
//! Brigadier passes `--home <dir>` (the main entry point turns it into
//! `BRIGADIER_CU_HOME` before anything else runs), so its socket, pid file,
//! config, journal and activity store all live under that app's data
//! directory. A helper started by hand falls back to a per-user directory.

use std::path::PathBuf;

/// Environment variable carrying the helper's home directory.
pub const HOME_ENV: &str = "BRIGADIER_CU_HOME";

/// The helper's home directory.
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os(HOME_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    default_home()
}

/// A helper started by hand: a per-user directory.
#[cfg(not(test))]
fn default_home() -> PathBuf {
    let user = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    user.join("Library/Application Support/Brigadier Computer Use")
}

#[cfg(test)]
fn default_home() -> PathBuf {
    test_home::get()
}

/// This crate's unit tests serve in-process and write their state (permissions,
/// activity, stopped apps) to [`home`]: one private folder per test process,
/// never the user's, removed when the process exits. A run that was killed
/// leaves its folder to the next run, which removes it.
#[cfg(test)]
mod test_home {
    #[cfg(unix)]
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    const PREFIX: &str = "brigadier-cu-test-home-";

    pub fn get() -> PathBuf {
        static HOME: OnceLock<PathBuf> = OnceLock::new();
        HOME.get_or_init(|| {
            let temp = std::env::temp_dir();
            #[cfg(unix)]
            remove_killed(&temp);
            let home = temp.join(format!("{PREFIX}{}", std::process::id()));
            let _ = std::fs::create_dir_all(&home);
            unsafe { libc::atexit(remove_own) };
            home
        })
        .clone()
    }

    extern "C" fn remove_own() {
        let _ = std::fs::remove_dir_all(
            std::env::temp_dir().join(format!("{PREFIX}{}", std::process::id())),
        );
    }

    #[cfg(unix)]
    fn remove_killed(temp: &Path) {
        let Ok(entries) = std::fs::read_dir(temp) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name
                .to_str()
                .and_then(|name| name.strip_prefix(PREFIX))
                .and_then(|pid| pid.parse::<libc::pid_t>().ok())
            else {
                continue;
            };
            if pid != std::process::id() as libc::pid_t && unsafe { libc::kill(pid, 0) } != 0 {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    #[test]
    fn a_test_process_keeps_its_state_in_a_private_folder_of_its_own() {
        let home = super::home();
        assert_eq!(
            home,
            std::env::temp_dir().join(format!("{PREFIX}{}", std::process::id()))
        );
        assert!(home.is_dir());
        assert_eq!(get(), home);
    }
}
