use std::ffi::OsString;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};

/// A process to spawn.
#[derive(Debug, Clone, Default)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    /// Start from an empty environment instead of inheriting the caller's; `env` is all the
    /// process gets.
    pub clear_env: bool,
    pub cwd: Option<PathBuf>,
    /// Runs at low OS priority (nice 10 on Unix, below normal on Windows), and so does
    /// everything it starts: work the user isn't waiting on (every worker).
    pub low_priority: bool,
}

impl SpawnSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// A `Command` with program, args, env and cwd applied and stdio set to null.
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        if self.clear_env {
            command.env_clear();
        }
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }

    /// Like [`SpawnSpec::command`] but with stdin, stdout and stderr piped.
    pub(crate) fn piped(&self) -> Command {
        let mut command = self.command();
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

/// A detached process started by [`crate::Processes::spawn_detached`].
///
/// Dropping it does not stop the process. Keep it and call [`DetachedChild::wait`] from a
/// dedicated thread to learn when the process exits and to reap it.
#[derive(Debug)]
pub struct DetachedChild {
    child: Child,
}

impl DetachedChild {
    pub(crate) fn new(child: Child) -> Self {
        Self { child }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Blocks until the process exits. Never call this on an async runtime thread.
    pub fn wait(mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }
}

/// An exclusive, advisory, cross-process lock held for the life of the daemon.
///
/// The OS releases it automatically if the process dies, so a crashed daemon never blocks the
/// next launch.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// Takes the lock, or returns `None` if another live process holds it.
    pub fn try_acquire(path: &Path) -> io::Result<Option<Self>> {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Error(err)) => return Err(err),
        }
        file.set_len(0)?;
        writeln!(file, "{}", std::process::id())?;
        Ok(Some(Self { _file: file }))
    }
}
