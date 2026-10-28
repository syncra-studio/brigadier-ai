//! A compiler at a time takes one of a machine's build slots. Every daemon uses the same
//! files under the user's home, independent of its data directory; they are created once,
//! never removed or replaced. The OS drops a dead wrapper's lock.
//!
//! The command is a second mode of `brigadier`, reached through each daemon's own link. Only the
//! wrapper owns the lock: its compiler and everything the compiler starts inherit no handle.
//! Compiler discovery goes straight through, so a busy build never delays a tool's probes.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

const NAME: &str = "brigadier-build-slot";
const RUSTC_CHAIN: &str = "BRIGADIER_BUILD_RUSTC_WRAPPER";
/// The shared pool's absolute path, retained when a CLI uses an account-specific home.
pub const POOL_ENV: &str = "BRIGADIER_BUILD_SLOTS_DIR";

/// The machine's compiler budget, shared by all sessions and workers.
pub fn count() -> usize {
    (std::thread::available_parallelism().map_or(1, usize::from) / 6).max(2)
}

fn root() -> io::Result<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(".brigadier"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no home for compiler slots"))
}

/// The persistent pool, shared by every daemon regardless of its data directory.
pub fn pool_path() -> io::Result<PathBuf> {
    Ok(root()?.join("build-slots"))
}

/// This daemon's compiler launcher. A missing or dangling link is not usable.
pub fn wrapper_path(data_dir: &Path) -> PathBuf {
    data_dir.join("build-tools").join(if cfg!(windows) {
        "brigadier-build-slot.exe"
    } else {
        NAME
    })
}

/// Prepares the launcher and persistent slot files before the daemon admits sessions.
pub fn prepare(data_dir: &Path, cli: &Path) -> io::Result<()> {
    prepare_in(&pool_path()?, data_dir, cli)
}

fn prepare_in(pool: &Path, data_dir: &Path, cli: &Path) -> io::Result<()> {
    prepare_pool(pool, count())?;
    let wrapper = wrapper_path(data_dir);
    fs::create_dir_all(wrapper.parent().expect("launcher folder"))?;
    #[cfg(unix)]
    if fs::read_link(&wrapper).ok().as_deref() == Some(cli) {
        return Ok(());
    }
    #[cfg(windows)]
    if same_contents(cli, &wrapper).unwrap_or(false) {
        return Ok(());
    }
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = wrapper.with_extension(format!("{}-{nonce}-new", std::process::id()));
    #[cfg(unix)]
    std::os::unix::fs::symlink(cli, &temp)?;
    #[cfg(windows)]
    fs::hard_link(cli, &temp).or_else(|_| fs::copy(cli, &temp).map(|_| ()))?;
    let result = fs::rename(&temp, &wrapper);
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(windows)]
fn same_contents(a: &Path, b: &Path) -> io::Result<bool> {
    fn hash(path: &Path) -> io::Result<blake3::Hash> {
        let mut hash = blake3::Hasher::new();
        hash.update_reader(File::open(path)?)?;
        Ok(hash.finalize())
    }
    Ok(fs::metadata(a)?.len() == fs::metadata(b)?.len() && hash(a)? == hash(b)?)
}

fn prepare_pool(dir: &Path, slots: usize) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for i in 0..slots {
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(format!("slot-{i}")))?;
    }
    Ok(())
}

// Rust's File uses flock on Unix and LockFileEx on Windows. Open handles are CLOEXEC /
// non-inheritable; Unix needs only read access, including inside a CLI's write sandbox.
fn acquire(dir: &Path, slots: usize) -> io::Result<File> {
    let mut files = Vec::with_capacity(slots);
    for i in 0..slots {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        options.write(true);
        let file = options.open(dir.join(format!("slot-{i}")))?;
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => files.push(file),
            Err(TryLockError::Error(err)) => return Err(err),
        }
    }
    // Spread waiters among slots instead of queuing every build behind slot zero.
    let file = files.swap_remove(std::process::id() as usize % slots);
    file.lock()?;
    Ok(file)
}

/// Ecosystem compiler launchers and job caps. An existing Rust wrapper runs inside ours;
/// every other variable is a default, including an explicitly empty user value.
pub fn environment(wrapper: &Path, get: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let Ok(pool) = pool_path() else {
        return Vec::new();
    };
    let wrapper = wrapper.to_string_lossy();
    let slots = count().to_string();
    // Go splits GOFLAGS, then splits the toolexec value again. Quote both levels when the
    // home folder has spaces; the launcher's path itself must remain one argument.
    let go_tool = go_quote(&wrapper);
    let defaults = [
        ("CMAKE_C_COMPILER_LAUNCHER", wrapper.to_string()),
        ("CMAKE_CXX_COMPILER_LAUNCHER", wrapper.to_string()),
        ("GOFLAGS", go_quote(&format!("-toolexec={go_tool}"))),
        ("CARGO_BUILD_JOBS", slots.clone()),
        ("CMAKE_BUILD_PARALLEL_LEVEL", slots.clone()),
        ("GRADLE_OPTS", format!("-Dorg.gradle.workers.max={slots}")),
    ];
    let mut env: Vec<_> = defaults
        .into_iter()
        .filter(|(key, _)| get(key).is_none())
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    let previous = get("RUSTC_WRAPPER").and_then(|previous| {
        if Path::new(&previous).file_stem() == Some(OsStr::new(NAME)) {
            // A dev daemon started inside a session inherits that daemon's launcher.
            // Keep the user's underlying wrapper, without taking a second compiler slot.
            get(RUSTC_CHAIN)
        } else {
            Some(previous)
        }
    });
    if let Some(previous) = previous
        && !previous.is_empty()
        && previous != wrapper
    {
        env.push((RUSTC_CHAIN.to_owned(), previous));
    }
    env.push(("RUSTC_WRAPPER".to_owned(), wrapper.into_owned()));
    env.push((POOL_ENV.to_owned(), pool.to_string_lossy().into_owned()));
    env
}

fn go_quote(value: &str) -> String {
    if !value.chars().any(char::is_whitespace) {
        value.to_owned()
    } else if !value.contains('\'') {
        format!("'{value}'")
    } else {
        format!("\"{value}\"")
    }
}

/// Dispatches before normal CLI parsing, without interpreting compiler arguments as UTF-8.
pub fn is_wrapper() -> bool {
    std::env::args_os()
        .next()
        .is_some_and(|arg| Path::new(&arg).file_stem() == Some(OsStr::new(NAME)))
}

fn rustc(tool: &OsStr) -> bool {
    Path::new(tool).file_stem() == Some(OsStr::new("rustc"))
        || ["RUSTC", "RUSTC_WORKSPACE_WRAPPER"]
            .iter()
            .any(|key| std::env::var_os(key).as_deref() == Some(tool))
}

fn probe(args: &[OsString]) -> bool {
    args.iter().any(|arg| {
        arg == "-vV"
            || arg == "--version"
            || arg == "--print"
            || arg.to_string_lossy().starts_with("--print=")
    }) || args
        .windows(2)
        .any(|pair| pair[0] == "--crate-name" && pair[1] == "___")
        || args.iter().any(|arg| arg == "--crate-name=___")
}

fn run() -> io::Result<ExitStatus> {
    let mut words = std::env::args_os().skip(1);
    let tool = words
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "compiler path required"))?;
    let args: Vec<_> = words.collect();
    let is_rustc = rustc(&tool);
    let mut command =
        if is_rustc && let Some(chain) = std::env::var_os(RUSTC_CHAIN).filter(|s| !s.is_empty()) {
            let mut command = Command::new(chain);
            command.arg(&tool);
            command
        } else {
            Command::new(&tool)
        };
    command.args(&args);
    if is_rustc && probe(&args) {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            return Err(command.exec());
        }
        #[cfg(windows)]
        return command.status();
    }
    let dir = std::env::var_os(POOL_ENV).map_or_else(pool_path, |dir| Ok(PathBuf::from(dir)))?;
    signals::install()?;
    let slot = acquire(&dir, count())?;
    signals::lower_priority();
    let status = signals::run(&mut command);
    drop(slot);
    status
}

/// Runs one compiler, preserving its exit code and, on Unix, its terminating signal.
/// This is only called by the binary's wrapper entry point.
pub fn main() -> ! {
    match run() {
        Ok(status) => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(signal) = status.signal() {
                    signals::exit_with_signal(signal);
                }
            }
            std::process::exit(status.code().unwrap_or(1));
        }
        Err(err) => {
            eprintln!("{NAME}: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(unix)]
mod signals {
    use super::*;
    use nix::sys::signal::{self, SigAction, SigHandler, SigSet, SigmaskHow, Signal};
    use nix::unistd::Pid;
    use std::os::unix::process::CommandExt;
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
    use std::time::Duration;

    static RUNNING: AtomicBool = AtomicBool::new(false);
    static PENDING: AtomicI32 = AtomicI32::new(0);
    const FORWARDED: [Signal; 4] = [
        Signal::SIGINT,
        Signal::SIGTERM,
        Signal::SIGHUP,
        Signal::SIGQUIT,
    ];

    extern "C" fn received(signal: i32) {
        if !RUNNING.load(Ordering::Relaxed) {
            // SAFETY: _exit is async-signal-safe. A waiter has no child; the OS closes its
            // lock handle, including if a signal arrived just after acquisition.
            #[allow(unsafe_code)]
            unsafe {
                libc::_exit(128 + signal)
            }
        }
        PENDING.store(signal, Ordering::Relaxed);
    }

    pub(super) fn install() -> io::Result<()> {
        let action = SigAction::new(
            SigHandler::Handler(received),
            signal::SaFlags::empty(),
            SigSet::empty(),
        );
        for sig in FORWARDED {
            // SAFETY: the handler only uses lock-free atomics and async-signal-safe _exit.
            #[allow(unsafe_code)]
            unsafe { signal::sigaction(sig, &action) }.map_err(io::Error::from)?;
        }
        Ok(())
    }

    pub(super) fn lower_priority() {
        // SAFETY: setpriority affects only this process; failure is deliberately ignored.
        #[allow(unsafe_code)]
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, 19)
        };
    }

    pub(super) fn run(command: &mut Command) -> io::Result<ExitStatus> {
        let mut blocked = SigSet::empty();
        for sig in FORWARDED {
            blocked.add(sig);
        }
        let mut previous = SigSet::empty();
        signal::pthread_sigmask(SigmaskHow::SIG_BLOCK, Some(&blocked), Some(&mut previous))?;
        // The compiler gets its own group, so a forwarded interrupt reaches its descendants.
        // Restore the caller's signal mask in the child, before exec, too.
        command.process_group(0);
        // SAFETY: sigprocmask is async-signal-safe; the closure uses only its captured mask.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(move || {
                signal::sigprocmask(SigmaskHow::SIG_SETMASK, Some(&previous), None)
                    .map_err(io::Error::from)
            });
        }
        let child = command.spawn();
        RUNNING.store(child.is_ok(), Ordering::Relaxed);
        signal::pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous), None)?;
        let mut child = child?;
        loop {
            if let Ok(sig) = Signal::try_from(PENDING.swap(0, Ordering::Relaxed)) {
                let _ = signal::killpg(Pid::from_raw(child.id() as i32), sig);
            }
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub(super) fn exit_with_signal(sig: i32) {
        if let Ok(sig) = Signal::try_from(sig) {
            // SAFETY: restores the default disposition in this single-purpose process.
            #[allow(unsafe_code)]
            unsafe {
                signal::sigaction(
                    sig,
                    &SigAction::new(
                        SigHandler::SigDfl,
                        signal::SaFlags::empty(),
                        SigSet::empty(),
                    ),
                )
            }
            .ok();
            let _ = signal::raise(sig);
        }
    }
}

#[cfg(windows)]
mod signals {
    use super::*;
    use windows_sys::Win32::System::{Console, Threading};

    #[allow(unsafe_code)]
    unsafe extern "system" fn received(event: u32) -> i32 {
        // The child shares our console and receives these events itself. Keep the wrapper
        // alive until it exits, so another compiler cannot take its slot prematurely.
        i32::from(event == Console::CTRL_C_EVENT || event == Console::CTRL_BREAK_EVENT)
    }

    pub(super) fn install() -> io::Result<()> {
        // SAFETY: static handler with the required ABI; no pointers into Rust storage.
        #[allow(unsafe_code)]
        if unsafe { Console::SetConsoleCtrlHandler(Some(received), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn lower_priority() {
        // SAFETY: the pseudo-handle names this process; setting priority may fail harmlessly.
        #[allow(unsafe_code)]
        unsafe {
            Threading::SetPriorityClass(
                Threading::GetCurrentProcess(),
                Threading::IDLE_PRIORITY_CLASS,
            )
        };
    }

    pub(super) fn run(command: &mut Command) -> io::Result<ExitStatus> {
        command.status()
    }
}

#[cfg(test)]
mod tests;
