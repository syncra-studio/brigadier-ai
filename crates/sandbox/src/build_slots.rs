//! A compiler at a time takes one of a machine's build slots. Every daemon uses the same
//! files under the user's home, independent of its data directory; they are created once,
//! never removed or replaced. The OS drops a dead compiler's lock.
//!
//! The command is a second mode of `brigadier`, reached through each daemon's own link. Unix
//! replaces the launcher with the compiler, carrying the lock through exec. Windows keeps
//! the compiler in a job that dies with the launcher, so no running compiler loses its slot.
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
const RUSTC_TOOL: &str = "BRIGADIER_BUILD_RUSTC";
const RUSTC_WORKSPACE: &str = "BRIGADIER_BUILD_RUSTC_WORKSPACE_WRAPPER";

/// Agent-only defaults and wrapper state, removed before starting a native terminal.
pub const INJECTED_ENV: &[&str] = &[
    "RUSTC_WRAPPER",
    RUSTC_CHAIN,
    RUSTC_TOOL,
    RUSTC_WORKSPACE,
    POOL_ENV,
    "CMAKE_C_COMPILER_LAUNCHER",
    "CMAKE_CXX_COMPILER_LAUNCHER",
    "GOFLAGS",
    "CARGO_BUILD_JOBS",
    "CMAKE_BUILD_PARALLEL_LEVEL",
    "GRADLE_OPTS",
];

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

// Rust's File uses flock on Unix and LockFileEx on Windows. Unix needs only read access,
// including inside a CLI's write sandbox. Execution decides which process owns the handle.
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
    environment_in(wrapper, None, get)
}

/// Compiler defaults for the folder the session or worker will run in.
pub fn environment_in(
    wrapper: &Path,
    cwd: Option<&Path>,
    get: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let Ok(pool) = pool_path() else {
        return Vec::new();
    };
    let wrapper = wrapper.to_string_lossy();
    let slots = count().to_string();
    // Go splits GOFLAGS, then splits the toolexec value again. Quote both levels when the
    // home folder has spaces; the launcher's path itself must remain one argument.
    let defaults = [
        ("CMAKE_C_COMPILER_LAUNCHER", wrapper.to_string()),
        ("CMAKE_CXX_COMPILER_LAUNCHER", wrapper.to_string()),
        ("CARGO_BUILD_JOBS", slots.clone()),
        ("CMAKE_BUILD_PARALLEL_LEVEL", slots.clone()),
        ("GRADLE_OPTS", format!("-Dorg.gradle.workers.max={slots}")),
    ];
    let mut env: Vec<_> = defaults
        .into_iter()
        .filter(|(key, _)| get(key).is_none())
        .map(|(key, value)| (key.to_owned(), value))
        .collect();
    // Go's quoted.Split has no escape syntax. Some valid filesystem names cannot
    // survive both parses; leave Go uncapped rather than supplying invalid flags.
    if get("GOFLAGS").is_none()
        && let Some(flags) =
            go_quote(&wrapper).and_then(|tool| go_quote(&format!("-toolexec={tool}")))
    {
        let previous = persistent_go_flags(cwd, &get).unwrap_or_default();
        // As with an explicit GOFLAGS, a user's existing tool launcher takes precedence.
        if !previous.split_whitespace().any(|flag| {
            flag.trim_start_matches(['\'', '"'])
                .starts_with("-toolexec=")
        }) {
            let flags = if previous.is_empty() {
                flags
            } else {
                format!("{previous} {flags}")
            };
            env.push(("GOFLAGS".to_owned(), flags));
        }
    }
    let config = cargo_config(cwd, &get);
    for (key, value) in [
        (RUSTC_TOOL, config.rustc),
        (RUSTC_WORKSPACE, config.workspace_wrapper),
    ] {
        if let Some(value) = value {
            env.push((key.to_owned(), value));
        }
    }
    let configured = get("RUSTC_WRAPPER")
        .or_else(|| get("CARGO_BUILD_RUSTC_WRAPPER"))
        .or(config.wrapper);
    let disabled = configured.as_deref() == Some("");
    let previous = configured.and_then(|previous| {
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
    if !disabled {
        env.push(("RUSTC_WRAPPER".to_owned(), wrapper.into_owned()));
    }
    env.push((POOL_ENV.to_owned(), pool.to_string_lossy().into_owned()));
    env
}

#[derive(Default)]
struct CargoConfig {
    wrapper: Option<String>,
    rustc: Option<String>,
    workspace_wrapper: Option<String>,
}

fn cargo_config(cwd: Option<&Path>, get: &impl Fn(&str) -> Option<String>) -> CargoConfig {
    let mut paths = Vec::new();
    if let Some(cwd) = cwd {
        let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_owned());
        paths.extend(
            cwd.ancestors()
                .map(|dir| cargo_config_file(&dir.join(".cargo"))),
        );
    }
    let home = get("HOME").or_else(|| get("USERPROFILE"));
    let cargo_home = get("CARGO_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|home| Path::new(&home).join(".cargo")));
    if let Some(home) = cargo_home {
        let home = if home.is_relative() {
            cwd.map_or(home.clone(), |cwd| cwd.join(&home))
        } else {
            home
        };
        paths.push(cargo_config_file(&home));
    }
    let mut config = CargoConfig::default();
    for path in paths {
        let Some(doc) = fs::read_to_string(&path)
            .ok()
            .and_then(|text| text.parse::<toml_edit::DocumentMut>().ok())
        else {
            continue;
        };
        for (key, output) in [
            ("rustc-wrapper", &mut config.wrapper),
            ("rustc", &mut config.rustc),
            ("rustc-workspace-wrapper", &mut config.workspace_wrapper),
        ] {
            if output.is_none()
                && let Some(value) = doc
                    .get("build")
                    .and_then(|build| build.get(key))
                    .and_then(toml_edit::Item::as_str)
            {
                // Cargo resolves executable paths against the parent of the .cargo folder;
                // bare executable names are searched on PATH instead.
                let value = if value.chars().any(std::path::is_separator)
                    && Path::new(value).is_relative()
                {
                    path.parent()
                        .and_then(Path::parent)
                        .unwrap_or(Path::new("."))
                        .join(value)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    value.to_owned()
                };
                *output = Some(value);
            }
        }
    }
    config
}

fn cargo_config_file(dir: &Path) -> PathBuf {
    let legacy = dir.join("config");
    if legacy.exists() {
        legacy
    } else {
        dir.join("config.toml")
    }
}

fn persistent_go_flags(
    cwd: Option<&Path>,
    get: &impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let path = if let Some(path) = get("GOENV").filter(|path| !path.is_empty()) {
        if path == "off" {
            return None;
        }
        let path = PathBuf::from(path);
        if path.is_relative() {
            cwd.map_or(path.clone(), |cwd| cwd.join(&path))
        } else {
            path
        }
    } else {
        go_config_dir(get)?.join("go/env")
    };
    let text = fs::read_to_string(path).ok()?;
    text.lines()
        .filter_map(|line| line.strip_prefix("GOFLAGS="))
        .next_back()
        .map(str::to_owned)
}

fn go_config_dir(get: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        get("HOME").map(|home| Path::new(&home).join("Library/Application Support"))
    }
    #[cfg(windows)]
    {
        get("APPDATA")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        if let Some(path) = get("XDG_CONFIG_HOME").filter(|path| !path.is_empty()) {
            let path = PathBuf::from(path);
            path.is_absolute().then_some(path)
        } else {
            get("HOME").map(|home| Path::new(&home).join(".config"))
        }
    }
}

fn go_quote(value: &str) -> Option<String> {
    if !value
        .chars()
        .any(|c| c.is_ascii_whitespace() || c == '\'' || c == '"')
    {
        Some(value.to_owned())
    } else if !value.contains('\'') {
        Some(format!("'{value}'"))
    } else if !value.contains('"') {
        Some(format!("\"{value}\""))
    } else {
        None
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
        || [
            "RUSTC",
            "RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            RUSTC_TOOL,
            RUSTC_WORKSPACE,
        ]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| matches_tool(&value, tool)))
}

fn matches_tool(configured: &OsStr, tool: &OsStr) -> bool {
    configured == tool
        || (Path::new(configured).components().count() == 1
            && Path::new(tool).file_name() == Some(configured))
        || match (
            Path::new(configured).canonicalize(),
            Path::new(tool).canonicalize(),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
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
    let slot = acquire(&dir, count())?;
    execution::run(&mut command, slot)
}

/// Runs one compiler, preserving its exit code and, on Unix, its terminating signal.
/// This is only called by the binary's wrapper entry point.
pub fn main() -> ! {
    match run() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(err) => {
            eprintln!("{NAME}: {err}");
            std::process::exit(1);
        }
    }
}

#[cfg(unix)]
mod execution {
    use super::*;
    use nix::fcntl::{FcntlArg, FdFlag, fcntl};
    use std::os::unix::process::CommandExt;

    pub(super) fn run(command: &mut Command, slot: File) -> io::Result<ExitStatus> {
        // The compiler replaces us: its pid, signals, status and lock have one lifetime.
        let mut flags = FdFlag::from_bits_truncate(fcntl(&slot, FcntlArg::F_GETFD)?);
        flags.remove(FdFlag::FD_CLOEXEC);
        fcntl(&slot, FcntlArg::F_SETFD(flags))?;
        // SAFETY: setpriority affects only this process; failure is deliberately ignored.
        #[allow(unsafe_code)]
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, 19)
        };
        Err(command.exec())
    }
}

#[cfg(windows)]
mod execution {
    use super::*;
    use std::os::windows::{io::AsRawHandle, process::CommandExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{Console, Diagnostics::ToolHelp, JobObjects, Threading},
    };

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: each handle is owned here and closed exactly once.
            #[allow(unsafe_code)]
            unsafe {
                CloseHandle(self.0)
            };
        }
    }

    #[allow(unsafe_code)]
    unsafe extern "system" fn received(event: u32) -> i32 {
        // The compiler shares our console and receives the event too. Wait for it instead
        // of closing the job first, giving it a chance to handle an interrupt normally.
        i32::from(event == Console::CTRL_C_EVENT || event == Console::CTRL_BREAK_EVENT)
    }

    fn job() -> io::Result<Handle> {
        // SAFETY: unnamed, non-inheritable job with default security.
        #[allow(unsafe_code)]
        let handle = unsafe { JobObjects::CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Handle(handle);
        let mut limits = JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JobObjects::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: the pointer and length describe the live limits structure of this class.
        #[allow(unsafe_code)]
        let set = unsafe {
            JobObjects::SetInformationJobObject(
                job.0,
                JobObjects::JobObjectExtendedLimitInformation,
                (&limits as *const JobObjects::JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn resume(pid: u32) -> io::Result<()> {
        // The child was created suspended, so it cannot compile before assignment to the
        // job. Its primary thread is the only thread until it starts executing.
        // SAFETY: snapshot flags have no pointers or caller-owned buffers.
        #[allow(unsafe_code)]
        let snapshot =
            unsafe { ToolHelp::CreateToolhelp32Snapshot(ToolHelp::TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = Handle(snapshot);
        let mut entry = ToolHelp::THREADENTRY32 {
            dwSize: std::mem::size_of::<ToolHelp::THREADENTRY32>() as u32,
            ..Default::default()
        };
        // SAFETY: entry's declared size matches its allocated buffer.
        #[allow(unsafe_code)]
        let mut found = unsafe { ToolHelp::Thread32First(snapshot.0, &mut entry) };
        while found != 0 {
            if entry.th32OwnerProcessID == pid {
                // SAFETY: only suspend/resume access to the thread id in this snapshot.
                #[allow(unsafe_code)]
                let thread = unsafe {
                    Threading::OpenThread(Threading::THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID)
                };
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let thread = Handle(thread);
                // SAFETY: the thread belongs to our suspended child, now in its job.
                #[allow(unsafe_code)]
                if unsafe { Threading::ResumeThread(thread.0) } == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            // SAFETY: entry remains a valid THREADENTRY32 buffer.
            #[allow(unsafe_code)]
            {
                found = unsafe { ToolHelp::Thread32Next(snapshot.0, &mut entry) };
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "compiler thread not found",
        ))
    }

    pub(super) fn run(command: &mut Command, _slot: File) -> io::Result<ExitStatus> {
        let job = job()?;
        // SAFETY: static callback; priority applies only to the current process.
        #[allow(unsafe_code)]
        unsafe {
            Console::SetConsoleCtrlHandler(Some(received), 1);
            Threading::SetPriorityClass(
                Threading::GetCurrentProcess(),
                Threading::IDLE_PRIORITY_CLASS,
            );
        }
        command.creation_flags(Threading::CREATE_SUSPENDED);
        let mut child = command.spawn()?;
        // SAFETY: both process and job handles remain open until the child is reaped.
        #[allow(unsafe_code)]
        let assigned =
            unsafe { JobObjects::AssignProcessToJobObject(job.0, child.as_raw_handle() as HANDLE) };
        let admitted = if assigned == 0 {
            Err(io::Error::last_os_error())
        } else {
            resume(child.id())
        };
        if let Err(err) = admitted {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        let status = child.wait();
        drop(job);
        status
    }
}

#[cfg(test)]
mod tests;
