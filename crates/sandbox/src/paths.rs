use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Where Brigadier keeps everything it owns. Nothing is ever stored inside a user's repo.
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Root data directory (`~/Library/Application Support/Brigadier` on macOS).
    pub data_dir: PathBuf,
    /// The SQLite event store.
    pub db_path: PathBuf,
    /// Content-addressed blob store root.
    pub blobs_dir: PathBuf,
    /// Structured daemon logs.
    pub logs_dir: PathBuf,
    /// Private runtime directory: instance lock, per-launch token, Unix socket.
    pub run_dir: PathBuf,
    /// Single-instance lock held by the running daemon.
    pub lock_path: PathBuf,
    /// Per-launch IPC secret, readable only by the current user.
    pub token_path: PathBuf,
    /// Where the daemon listens.
    pub ipc_endpoint: IpcEndpoint,
    /// Short id of this data directory (a hash of its path): tells apart what different data
    /// directories' daemons create in shared places (the socket fallback folder, session temp
    /// folders).
    pub instance: String,
    /// Where each session gets its one folder, `<sessions_dir>/<short id>/` ([`sessions_dir`]).
    pub sessions_dir: PathBuf,
}

/// The local IPC endpoint. There is never a TCP listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcEndpoint {
    /// Unix domain socket path.
    UnixSocket(PathBuf),
    /// Windows named pipe name, without the `\\.\pipe\` prefix.
    NamedPipe(String),
}

/// The file that marks a folder Brigadier created in a shared place (a session's temp folder
/// under `/tmp`) as this data directory's: it holds [`AppPaths::owner_marker`]. Cleanup outside
/// the data directory only ever takes folders that carry its own marker or are recorded in its
/// cleanup ledger.
pub const OWNER_MARKER: &str = ".brigadier-owner";

/// `sun_path` is 104 bytes on macOS and 108 on Linux, including the terminating NUL.
#[cfg(unix)]
const MAX_SOCKET_PATH: usize = 100;

impl AppPaths {
    /// The locations for the data directory `data_dir` (made absolute; not created).
    pub fn resolve(data_dir: PathBuf) -> Result<Self> {
        let data_dir = absolute(data_dir)?;
        let run_dir = data_dir.join("run");
        // Different data directories (dev, smoke check, release) get separate daemons.
        let instance =
            blake3::hash(data_dir.as_os_str().as_encoded_bytes()).to_hex()[..12].to_owned();

        #[cfg(unix)]
        let ipc_endpoint = {
            let preferred = run_dir.join("brigadierd.sock");
            if preferred.as_os_str().len() <= MAX_SOCKET_PATH {
                IpcEndpoint::UnixSocket(preferred)
            } else {
                // Long home directories overflow sun_path; fall back to a short private dir.
                let uid = nix::unistd::getuid();
                IpcEndpoint::UnixSocket(
                    std::env::temp_dir()
                        .join(format!("brigadier-{uid}-{instance}"))
                        .join("d.sock"),
                )
            }
        };
        #[cfg(windows)]
        let ipc_endpoint = IpcEndpoint::NamedPipe(format!("brigadier-{instance}"));

        Ok(Self {
            sessions_dir: sessions_dir(&data_dir, &instance),
            db_path: data_dir.join("brigadier.db"),
            blobs_dir: data_dir.join("blobs"),
            logs_dir: data_dir.join("logs"),
            lock_path: run_dir.join("brigadierd.lock"),
            token_path: run_dir.join("ipc.token"),
            run_dir,
            ipc_endpoint,
            data_dir,
            instance,
        })
    }

    /// What [`OWNER_MARKER`] holds for this data directory: its instance id and path.
    pub fn owner_marker(&self) -> String {
        format!("{}\n{}\n", self.instance, self.data_dir.display())
    }

    /// The directory that holds the Unix socket, if it is not the run directory.
    pub fn socket_dir(&self) -> Option<&Path> {
        match &self.ipc_endpoint {
            IpcEndpoint::UnixSocket(path) => path.parent().filter(|dir| *dir != self.run_dir),
            IpcEndpoint::NamedPipe(_) => None,
        }
    }
}

fn absolute(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::path::absolute(path)?)
    }
}

/// The root of the session folders of the data directory `data_dir` (whose instance id is
/// `instance`): short and free of spaces, so what a session's commands make there (a Unix
/// socket's path holds at most 104 bytes on macOS) fits.
/// - The installed app's: `~/.brigadier/s`.
/// - Another data directory's (a dev build, a test identity): `~/.brigadier/<instance>/s`, so
///   two apps never share one.
/// - A data directory in a temporary folder (tests, scratch runs): `<data_dir>-s` beside it, so
///   they leave nothing in the home folder and still keep sessions out of the data directory.
pub fn sessions_dir(data_dir: &Path, instance: &str) -> PathBuf {
    // Resolved through its deepest folder that exists (`/var` is `/private/var` on macOS).
    let resolved = |path: &Path| {
        let mut rest = Vec::new();
        let mut base = path;
        loop {
            if let Ok(real) = base.canonicalize() {
                return rest.iter().rev().fold(real, |path, part| path.join(part));
            }
            match (base.parent(), base.file_name()) {
                (Some(parent), Some(name)) => {
                    rest.push(name.to_owned());
                    base = parent;
                }
                _ => return path.to_owned(),
            }
        }
    };
    let data = resolved(data_dir);
    let temporary = [std::env::temp_dir(), PathBuf::from("/tmp")]
        .iter()
        .any(|temp| data.starts_with(resolved(temp)));
    let home = dirs::home_dir();
    match home {
        Some(home) if !temporary => {
            let root = home.join(".brigadier");
            if default_data_dir().is_ok_and(|installed| resolved(&installed) == data) {
                root.join("s")
            } else {
                root.join(instance).join("s")
            }
        }
        _ => {
            let mut beside = data_dir.as_os_str().to_owned();
            beside.push("-s");
            PathBuf::from(beside)
        }
    }
}

/// Where the installed app keeps its data when `BRIGADIER_DATA_DIR` names no other folder.
pub fn default_data_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let dir = dirs::data_dir().map(|dir| dir.join("Brigadier"));
    #[cfg(target_os = "linux")]
    let dir = dirs::data_dir().map(|dir| dir.join("brigadier"));
    // A folder of its own in `%LOCALAPPDATA%\Brigadier`, where the per-user installer puts the
    // program files: its uninstaller leaves the data, and Uninstall Brigadier… moves the data
    // without the running programs.
    #[cfg(windows)]
    let dir = dirs::data_local_dir().map(|dir| dir.join("Brigadier").join("Data"));
    dir.ok_or(Error::NoDataDir)
}

/// Chooses an app's data directory; `override_dir` is the value of `BRIGADIER_DATA_DIR`.
pub(crate) fn choose_data_dir(identifier: &str, override_dir: Option<PathBuf>) -> Result<PathBuf> {
    match override_dir {
        Some(dir) if !dir.as_os_str().is_empty() => Ok(dir),
        _ if identifier == crate::APP_ID => default_data_dir(),
        _ => {
            #[cfg(windows)]
            let base = dirs::data_local_dir();
            #[cfg(not(windows))]
            let base = dirs::data_dir();
            base.map(|dir| dir.join(identifier)).ok_or(Error::NoDataDir)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_directory_follows_bundle_identity_unless_overridden() {
        let installed = default_data_dir().unwrap();
        let custom_id = "ai.brigadier.issues2-w35";
        assert_eq!(choose_data_dir(crate::APP_ID, None).unwrap(), installed);
        // On Windows the installed app's data is a level down (`Brigadier\Data`).
        let base = if cfg!(windows) {
            installed.parent().and_then(Path::parent)
        } else {
            installed.parent()
        };
        assert_eq!(
            choose_data_dir(custom_id, None).unwrap(),
            base.unwrap().join(custom_id)
        );
        let override_dir = PathBuf::from("/tmp/20261018-1945-issues-run-2-w35-data");
        for identifier in [crate::APP_ID, custom_id] {
            assert_eq!(
                choose_data_dir(identifier, Some(override_dir.clone())).unwrap(),
                override_dir
            );
        }
    }
}

#[cfg(test)]
mod session_root_tests {
    use super::*;

    #[test]
    fn sessions_live_in_a_short_folder_of_the_home_folder_and_tests_keep_theirs() {
        let home = dirs::home_dir().unwrap().join(".brigadier");
        let installed = default_data_dir().unwrap();
        assert_eq!(sessions_dir(&installed, "abc"), home.join("s"));
        let dev = installed.parent().unwrap().join("ai.brigadier.dev-test");
        assert_eq!(
            sessions_dir(&dev, "0123456789ab"),
            home.join("0123456789ab").join("s")
        );
        let temp = std::env::temp_dir().join("brigadier-paths-test");
        assert_eq!(
            sessions_dir(&temp, "x"),
            std::env::temp_dir().join("brigadier-paths-test-s")
        );
        assert_eq!(
            sessions_dir(Path::new("/tmp/brigadier-data"), "x"),
            Path::new("/tmp/brigadier-data-s")
        );
    }
}
