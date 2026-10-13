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
    let user = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    user.join("Library/Application Support/Brigadier Computer Use")
}
