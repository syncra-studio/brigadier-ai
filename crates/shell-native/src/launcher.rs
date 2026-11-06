//! Launches `brigadierd` genuinely detached from the app (own session / process group, no
//! inherited stdio), so it outlives the app and keeps long sessions running.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use brigadier_sandbox::{Platform, SpawnSpec};
use tokio::sync::{Mutex, watch};

/// Don't relaunch more often than this while a fresh daemon is still starting up.
const RELAUNCH_COOLDOWN: Duration = Duration::from_secs(3);

/// A daemon this app launched: whether it has exited yet, and how.
#[derive(Clone)]
pub struct Launched {
    /// None while it runs; then its exit code (None when it has none, killed by a signal).
    exit: watch::Receiver<Option<Option<i32>>>,
}

impl Launched {
    fn new() -> (watch::Sender<Option<Option<i32>>>, Self) {
        let (tx, exit) = watch::channel(None);
        (tx, Self { exit })
    }

    /// It exited with success: a daemon exits so at once when another holds the data folder's
    /// instance lock.
    pub fn exited_ok(&self) -> bool {
        *self.exit.borrow() == Some(Some(0))
    }

    /// One still running.
    #[cfg(test)]
    pub fn running() -> Self {
        Self::new().1
    }

    /// One that already exited with `code`.
    #[cfg(test)]
    pub fn exited(code: i32) -> Self {
        let (tx, launched) = Self::new();
        tx.send_replace(Some(Some(code)));
        launched
    }
}

pub struct Launcher {
    platform: Arc<dyn Platform>,
    extra_env: Vec<(String, String)>,
    /// Where `brigadierd` is, when not beside this executable (a dev run of the Electron app).
    sidecar_dir: Option<PathBuf>,
    last_launch: Mutex<Option<Instant>>,
    /// In tests, what a launch gives instead of starting a process.
    #[cfg(test)]
    fake: Option<Box<dyn Fn() -> Launched + Send + Sync>>,
}

impl Launcher {
    pub fn new(platform: Arc<dyn Platform>, extra_env: Vec<(String, String)>) -> Self {
        Self {
            platform,
            extra_env,
            sidecar_dir: None,
            last_launch: Mutex::new(None),
            #[cfg(test)]
            fake: None,
        }
    }

    /// Finds `brigadierd` in `dir` instead of beside this executable.
    pub fn with_sidecar_dir(mut self, dir: PathBuf) -> Self {
        self.sidecar_dir = Some(dir);
        self
    }

    /// A launcher that starts no process: each launch gives what `launch` returns.
    #[cfg(test)]
    pub fn fake(
        platform: Arc<dyn Platform>,
        launch: impl Fn() -> Launched + Send + Sync + 'static,
    ) -> Self {
        Self {
            fake: Some(Box::new(launch)),
            ..Self::new(platform, vec![])
        }
    }

    /// The sidecar sits next to the app executable, both in dev and inside the bundle, unless
    /// this launcher was told otherwise.
    fn sidecar(&self) -> std::io::Result<PathBuf> {
        let exe = std::env::current_exe()?;
        let dir = match &self.sidecar_dir {
            Some(dir) => dir.as_path(),
            None => exe
                .parent()
                .ok_or_else(|| std::io::Error::other("executable has no parent directory"))?,
        };
        Ok(dir.join(format!("brigadierd{}", std::env::consts::EXE_SUFFIX)))
    }

    /// Starts the daemon unless one was launched moments ago (then None). The daemon's own
    /// instance lock makes a redundant launch harmless: the second process exits immediately,
    /// which the returned [`Launched`] tells.
    pub async fn ensure_launched(&self) -> Result<Option<Launched>, String> {
        let mut last = self.last_launch.lock().await;
        if last.is_some_and(|at| at.elapsed() < RELAUNCH_COOLDOWN) {
            return Ok(None);
        }
        #[cfg(test)]
        if let Some(fake) = &self.fake {
            *last = Some(Instant::now());
            return Ok(Some(fake()));
        }
        let program = self
            .sidecar()
            .map_err(|err| format!("locating brigadierd: {err}"))?;
        let mut spec = SpawnSpec::new(program)
            .arg("--data-dir")
            .arg(self.platform.paths().data_dir.as_os_str());
        for (key, value) in &self.extra_env {
            spec = spec.env(key, value);
        }
        let platform = self.platform.clone();
        let child = tokio::task::spawn_blocking(move || platform.processes().spawn_detached(&spec))
            .await
            .map_err(|err| err.to_string())?
            .map_err(|err| format!("launching brigadierd: {err}"))?;
        *last = Some(Instant::now());
        let pid = child.pid();
        tracing::info!(pid, "launched brigadierd");

        // Reap the process when it exits so it never lingers as a zombie while the app runs.
        let (exit, launched) = Launched::new();
        std::thread::Builder::new()
            .name("brigadierd-reaper".into())
            .spawn(move || match child.wait() {
                Ok(status) => {
                    tracing::info!(pid, %status, "brigadierd exited");
                    exit.send_replace(Some(status.code()));
                }
                Err(err) => tracing::warn!(pid, error = %err, "waiting for brigadierd failed"),
            })
            .map_err(|err| err.to_string())?;
        Ok(Some(launched))
    }
}
