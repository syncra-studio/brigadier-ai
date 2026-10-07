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

/// Where the installed app keeps its data when `BRIGADIER_DATA_DIR` names no other folder.
pub fn default_data_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    let dir = dirs::data_dir().map(|dir| dir.join("Brigadier"));
    #[cfg(target_os = "linux")]
    let dir = dirs::data_dir().map(|dir| dir.join("brigadier"));
    #[cfg(windows)]
    let dir = dirs::data_local_dir().map(|dir| dir.join("Brigadier"));
    dir.ok_or(Error::NoDataDir)
}
