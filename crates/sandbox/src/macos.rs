//! macOS: Keychain credentials, Seatbelt sandbox, login-shell environment.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use security_framework::passwords;

use crate::machine::{heat_from_thermal_state, memory_from_pressure_level};
use crate::unix::{self, UnixPrivateFs};
use crate::{
    APP_ID, AppPaths, CredentialStore, DetachedChild, Error, Machine, MachineLoad, Platform,
    PrivateFs, Processes, Result, Sandbox, SandboxPolicy, Shell, SpawnSpec,
};

pub(crate) struct MacOs {
    paths: AppPaths,
}

impl MacOs {
    pub(crate) fn new(paths: AppPaths) -> Self {
        Self { paths }
    }
}

impl Platform for MacOs {
    fn name(&self) -> &'static str {
        "macos"
    }
    fn paths(&self) -> &AppPaths {
        &self.paths
    }
    fn private_fs(&self) -> &dyn PrivateFs {
        &UnixPrivateFs
    }
    fn processes(&self) -> &dyn Processes {
        &MacProcesses
    }
    fn credentials(&self) -> &dyn CredentialStore {
        &Keychain
    }
    fn shell(&self) -> &dyn Shell {
        &LoginShell
    }
    fn sandbox(&self) -> &dyn Sandbox {
        &Seatbelt
    }
    fn machine(&self) -> &dyn Machine {
        &MacMachine
    }
}

struct MacProcesses;

impl Processes for MacProcesses {
    fn spawn_detached(&self, spec: &SpawnSpec) -> Result<DetachedChild> {
        unix::spawn_detached(spec)
    }
    fn piped_command(&self, spec: &SpawnSpec) -> std::process::Command {
        unix::piped_command(spec)
    }
    fn is_alive(&self, pid: u32) -> bool {
        unix::is_alive(pid)
    }
    fn is_zombie(&self, pid: u32) -> bool {
        is_zombie(pid)
    }
    fn terminate(&self, pid: u32) -> Result<()> {
        unix::terminate(pid)
    }
    fn kill_tree(&self, pid: u32) -> Result<()> {
        unix::kill_tree(pid, child_pids)
    }
    fn kill_group(&self, pid: u32) -> Result<()> {
        unix::kill_group(pid)
    }
    fn descendants(&self, pid: u32) -> Result<Vec<u32>> {
        Ok(unix::descendants(pid, child_pids))
    }
    fn children(&self, pid: u32) -> Result<Vec<u32>> {
        Ok(child_pids(pid))
    }
    fn group_of(&self, pid: u32) -> Option<u32> {
        unix::group_of(pid)
    }
    fn command_line(&self, pid: u32) -> Option<Vec<String>> {
        command_line(pid)
    }
    fn suspend(&self, pid: u32) -> Result<()> {
        unix::suspend(pid)
    }
    fn resume(&self, pid: u32) -> Result<()> {
        unix::resume(pid)
    }
    fn cpu_time_ms(&self, pid: u32) -> Option<u64> {
        let pid = libc::c_int::try_from(pid).ok()?;
        let size = std::mem::size_of::<libc::proc_taskinfo>();
        let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
        // SAFETY: `info` is a writable buffer of exactly `size` bytes, which is what
        // PROC_PIDTASKINFO fills; the return value is checked before `info` is read.
        #[allow(unsafe_code)]
        let (written, info) = unsafe {
            let written = libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast(),
                size as libc::c_int,
            );
            (written, info.assume_init())
        };
        if written != size as libc::c_int {
            return None;
        }
        // The totals count Mach time units (nanoseconds only on Intel).
        let (numer, denom) = timebase();
        let ticks = u128::from(info.pti_total_user + info.pti_total_system);
        u64::try_from(ticks * u128::from(numer) / u128::from(denom) / 1_000_000).ok()
    }
    fn in_dir(&self, dir: &Path) -> Result<Vec<u32>> {
        let dir = dir.canonicalize()?;
        let own = std::process::id();
        Ok(all_pids()?
            .into_iter()
            .filter(|pid| *pid != own && *pid > 1)
            .filter(|pid| working_dir(*pid).is_some_and(|cwd| cwd.starts_with(&dir)))
            .collect())
    }
    fn start_time_ms(&self, pid: u32) -> Result<f64> {
        // `kinfo_proc` holds the start time `PROC_PIDTBSDINFO` reports, but unlike that it is
        // readable for another user's process and for a zombie, so a reused PID shows.
        let info = kinfo_proc(pid)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such process"))?;
        let field = |at: usize, len: usize| &info[at..at + len];
        let seconds = i64::from_ne_bytes(field(P_STARTTIME_SEC, 8).try_into().unwrap());
        let micros = i32::from_ne_bytes(field(P_STARTTIME_USEC, 4).try_into().unwrap());
        Ok(seconds as f64 * 1000.0 + f64::from(micros) / 1000.0)
    }
}

/// A process's arguments, per `KERN_PROCARGS2`: the argument count, the executable's path,
/// padding, then the arguments, each ended by a NUL.
fn command_line(pid: u32) -> Option<Vec<String>> {
    let pid = libc::c_int::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let mut max: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `max` is a writable `c_int` and `size` says so; the result is checked.
    #[allow(unsafe_code)]
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            2,
            (&raw mut max).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 || max <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; usize::try_from(max).ok()?];
    let mut size = buffer.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: `buffer` is writable for `size` bytes and the call writes at most that many,
    // updating `size`; the result is checked before `buffer` is read.
    #[allow(unsafe_code)]
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if read != 0 {
        return None;
    }
    buffer.truncate(size);
    parse_procargs(&buffer)
}

fn parse_procargs(buffer: &[u8]) -> Option<Vec<String>> {
    let count = usize::try_from(i32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?)).ok()?;
    let rest = buffer.get(4..)?;
    // The executable's path, then the NULs padding it.
    let path_end = rest.iter().position(|byte| *byte == 0)?;
    let start = path_end + rest[path_end..].iter().position(|byte| *byte != 0)?;
    let args = rest[start..]
        .split(|byte| *byte == 0)
        .take(count)
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect::<Vec<_>>();
    (!args.is_empty()).then_some(args)
}

/// The Mach timebase: Mach time units times `numer / denom` are nanoseconds. (libc marks its
/// Mach bindings deprecated in favour of another crate; this one call doesn't warrant it.)
#[allow(deprecated)]
fn timebase() -> (u32, u32) {
    static TIMEBASE: std::sync::OnceLock<(u32, u32)> = std::sync::OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = libc::mach_timebase_info { numer: 0, denom: 0 };
        // SAFETY: `info` is a writable `mach_timebase_info`, which is all the call fills.
        #[allow(unsafe_code)]
        let read = unsafe { libc::mach_timebase_info(&mut info) };
        if read == 0 && info.denom != 0 {
            (info.numer, info.denom)
        } else {
            (1, 1)
        }
    })
}

/// Whether `pid` is a zombie, per its `kinfo_proc` (`proc_pidinfo` fails for a zombie, as it
/// has no task any more).
fn is_zombie(pid: u32) -> bool {
    kinfo_proc(pid).is_some_and(|info| u32::from(info[P_STAT]) == libc::SZOMB)
}

// `struct kinfo_proc` (libc does not define it on Apple platforms): 648 bytes on 64-bit
// macOS, starting with `kp_proc`, whose `p_starttime` (a `timeval`: 64-bit seconds, then
// 32-bit microseconds) is at offset 0 and `p_stat` at offset 36.
const KINFO_PROC_SIZE: usize = 648;
const P_STARTTIME_SEC: usize = 0;
const P_STARTTIME_USEC: usize = 8;
const P_STAT: usize = 36;

/// `pid`'s `kinfo_proc`, per `KERN_PROC_PID`: readable for any user's process, zombies
/// included; `None` once it is reaped.
fn kinfo_proc(pid: u32) -> Option<[u8; KINFO_PROC_SIZE]> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let mut buffer = [0u8; KINFO_PROC_SIZE];
    let mut size = KINFO_PROC_SIZE;
    let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_PID, pid];
    // SAFETY: `buffer` is writable for `size` bytes and the call writes at most that many,
    // updating `size`; the result is checked before `buffer` is read.
    #[allow(unsafe_code)]
    let read = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            4,
            buffer.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    // No such process leaves `size` at 0.
    (read == 0 && size == KINFO_PROC_SIZE).then_some(buffer)
}

/// Every process id on the machine, per `proc_listallpids`.
fn all_pids() -> Result<Vec<u32>> {
    let mut capacity = 4096;
    loop {
        let mut pids = vec![0 as libc::pid_t; capacity];
        let bytes = (capacity * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `pids` is a writable buffer of exactly `bytes` bytes; the call returns how
        // many pids it wrote, never more than fit.
        #[allow(unsafe_code)]
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        let count = usize::try_from(count).map_err(|_| io::Error::last_os_error())?;
        if count < capacity {
            pids.truncate(count);
            return Ok(pids
                .into_iter()
                .filter_map(|pid| u32::try_from(pid).ok())
                .collect());
        }
        capacity *= 2;
    }
}

/// A process's working directory, per `PROC_PIDVNODEPATHINFO`. Readable for the current
/// user's processes only; `None` otherwise or once it has exited.
fn working_dir(pid: u32) -> Option<PathBuf> {
    let pid = libc::c_int::try_from(pid).ok()?;
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    // SAFETY: `info` is a writable buffer of exactly `size` bytes, which is what
    // PROC_PIDVNODEPATHINFO fills; the return value is checked before `info` is read.
    #[allow(unsafe_code)]
    let (written, info) = unsafe {
        let written = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size as libc::c_int,
        );
        (written, info.assume_init())
    };
    if written != size as libc::c_int {
        return None;
    }
    let raw = info.pvi_cdir.vip_path.as_flattened();
    let bytes: Vec<u8> = raw
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect();
    if bytes.is_empty() {
        return None;
    }
    Some(PathBuf::from(OsString::from_vec(bytes)))
}

/// A process's children, per `proc_listchildpids`.
fn child_pids(pid: u32) -> Vec<u32> {
    let Ok(pid) = i32::try_from(pid) else {
        return Vec::new();
    };
    let mut capacity = 64;
    loop {
        let mut pids = vec![0 as libc::pid_t; capacity];
        let bytes = (capacity * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `pids` is a writable buffer of exactly `bytes` bytes; the call returns how
        // many pids it wrote, never more than fit.
        #[allow(unsafe_code)]
        let count = unsafe { libc::proc_listchildpids(pid, pids.as_mut_ptr().cast(), bytes) };
        let Ok(count) = usize::try_from(count) else {
            return Vec::new();
        };
        // A full buffer may have cut the list short.
        if count < capacity {
            pids.truncate(count);
            return pids
                .into_iter()
                .filter_map(|pid| u32::try_from(pid).ok())
                .collect();
        }
        capacity *= 4;
    }
}

/// The kernel's `audit_token_t`: who a process is, fixed for its lifetime (unlike a pid).
#[repr(C)]
struct AuditToken {
    val: [u32; 8],
}

// libsystem_sandbox. With no operation, both ask whether the process is sandboxed at all
// (1 when it is). Declared in its private header; stable since macOS 10.7 and 10.9.
#[allow(unsafe_code)]
unsafe extern "C" {
    fn sandbox_check(
        pid: libc::pid_t,
        operation: *const libc::c_char,
        filter: libc::c_int,
        ...
    ) -> libc::c_int;
    fn sandbox_check_by_audit_token(
        token: AuditToken,
        operation: *const libc::c_char,
        filter: libc::c_int,
        ...
    ) -> libc::c_int;
}

/// `SANDBOX_FILTER_NONE`: the check names no path or service.
const SANDBOX_FILTER_NONE: libc::c_int = 0;

/// See [`crate::peer_confined`]. The peer is named by the audit token the kernel recorded when
/// it connected, so a pid reused since cannot stand in for it.
pub(crate) fn peer_confined(socket: std::os::fd::BorrowedFd<'_>) -> bool {
    use std::os::fd::AsRawFd;
    let mut token = AuditToken { val: [0; 8] };
    let mut len = std::mem::size_of::<AuditToken>() as libc::socklen_t;
    // SAFETY: `token` is a writable buffer of `len` bytes, which is what LOCAL_PEERTOKEN
    // fills; the result and the length written are checked before the token is used.
    #[allow(unsafe_code)]
    let read = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            (&raw mut token).cast(),
            &mut len,
        )
    };
    if read != 0 || len as usize != std::mem::size_of::<AuditToken>() {
        return false;
    }
    // SAFETY: plain queries with a null operation and no variadic arguments, as documented for
    // SANDBOX_FILTER_NONE; they read nothing but their arguments.
    #[allow(unsafe_code)]
    let (peer, own) = unsafe {
        (
            sandbox_check_by_audit_token(token, std::ptr::null(), SANDBOX_FILTER_NONE),
            sandbox_check(libc::getpid(), std::ptr::null(), SANDBOX_FILTER_NONE),
        )
    };
    peer == 1 && own != 1
}

/// `ProcessInfo.thermalState` and the kernel's memory-pressure level.
struct MacMachine;

impl Machine for MacMachine {
    fn load(&self) -> MachineLoad {
        MachineLoad {
            heat: thermal_state()
                .map(heat_from_thermal_state)
                .unwrap_or_default(),
            memory: memory_pressure_level()
                .map(memory_from_pressure_level)
                .unwrap_or_default(),
        }
    }
}

#[allow(unsafe_code)]
#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

// The Objective-C runtime, to read `[[NSProcessInfo processInfo] thermalState]`.
#[allow(unsafe_code)]
#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn sel_registerName(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    fn objc_msgSend();
}

/// `ProcessInfo.processInfo.thermalState`'s raw value (0 nominal … 3 critical).
fn thermal_state() -> Option<isize> {
    type Object = *mut std::ffi::c_void;
    // SAFETY: `objc_msgSend` is called through the exact signature of each method it sends
    // to: `+[NSProcessInfo processInfo]` returns an object, `-thermalState` an NSInteger. The
    // class and the shared instance are checked for null before use; the shared instance is
    // never released.
    #[allow(unsafe_code)]
    unsafe {
        let class = objc_getClass(c"NSProcessInfo".as_ptr());
        if class.is_null() {
            return None;
        }
        let send_object = std::mem::transmute::<
            unsafe extern "C" fn(),
            unsafe extern "C" fn(Object, Object) -> Object,
        >(objc_msgSend);
        let info = send_object(class, sel_registerName(c"processInfo".as_ptr()));
        if info.is_null() {
            return None;
        }
        let send_integer = std::mem::transmute::<
            unsafe extern "C" fn(),
            unsafe extern "C" fn(Object, Object) -> isize,
        >(objc_msgSend);
        Some(send_integer(
            info,
            sel_registerName(c"thermalState".as_ptr()),
        ))
    }
}

/// `kern.memorystatus_vm_pressure_level`: 1 normal, 2 warning, 4 critical.
fn memory_pressure_level() -> Option<i32> {
    let mut level: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: `level` is a writable `c_int` and `size` says so; the result is checked.
    #[allow(unsafe_code)]
    let read = unsafe {
        libc::sysctlbyname(
            c"kern.memorystatus_vm_pressure_level".as_ptr(),
            (&raw mut level).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (read == 0).then_some(level)
}

struct Keychain;

/// `errSecItemNotFound`
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

impl CredentialStore for Keychain {
    fn set(&self, account: &str, secret: &[u8]) -> Result<()> {
        passwords::set_generic_password(APP_ID, account, secret)
            .map_err(|err| Error::Credentials(err.to_string()))
    }

    fn get(&self, account: &str) -> Result<Option<Vec<u8>>> {
        match passwords::get_generic_password(APP_ID, account) {
            Ok(secret) => Ok(Some(secret)),
            Err(err) if err.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(err) => Err(Error::Credentials(err.to_string())),
        }
    }

    fn delete(&self, account: &str) -> Result<()> {
        match passwords::delete_generic_password(APP_ID, account) {
            Ok(()) => Ok(()),
            Err(err) if err.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(err) => Err(Error::Credentials(err.to_string())),
        }
    }
}

struct LoginShell;

const ENV_MARKER: &[u8] = b"\0__BRIGADIER_ENV__\0";
const SHELL_TIMEOUT: Duration = Duration::from_secs(10);

impl Shell for LoginShell {
    fn login_shell(&self) -> Result<PathBuf> {
        let uid = nix::unistd::getuid();
        match nix::unistd::User::from_uid(uid) {
            Ok(Some(user)) if !user.shell.as_os_str().is_empty() => Ok(user.shell),
            _ => std::env::var_os("SHELL")
                .map(PathBuf::from)
                .ok_or_else(|| Error::Shell("no login shell configured".into())),
        }
    }

    fn login_environment(&self) -> Result<BTreeMap<OsString, OsString>> {
        let shell = self.login_shell()?;
        // A marker separates whatever the user's rc files print from the environment dump.
        let script = "printf '\\0__BRIGADIER_ENV__\\0'; exec /usr/bin/env -0";
        let mut child = Command::new(&shell)
            .args(["-l", "-i", "-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| Error::Shell(format!("spawning {}: {err}", shell.display())))?;
        let pid = child.id();
        let mut stdout = child.stdout.take().expect("stdout is piped");

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let result = stdout.read_to_end(&mut out).map(|_| out);
            let _ = tx.send(result);
        });
        let output = match rx.recv_timeout(SHELL_TIMEOUT) {
            Ok(output) => output?,
            Err(_) => {
                let _ = unix::kill_tree(pid, child_pids);
                let _ = child.wait();
                return Err(Error::Shell(format!(
                    "{} did not finish within {SHELL_TIMEOUT:?}",
                    shell.display()
                )));
            }
        };
        let _ = child.wait();

        let start = output
            .windows(ENV_MARKER.len())
            .rposition(|window| window == ENV_MARKER)
            .ok_or_else(|| Error::Shell("login shell produced no environment".into()))?;
        Ok(output[start + ENV_MARKER.len()..]
            .split(|byte| *byte == 0)
            .filter_map(|entry| {
                let eq = entry.iter().position(|byte| *byte == b'=')?;
                Some((
                    OsString::from_vec(entry[..eq].to_vec()),
                    OsString::from_vec(entry[eq + 1..].to_vec()),
                ))
            })
            .collect())
    }
}

struct Seatbelt;

/// `path` as Seatbelt matches it: symlinks such as /tmp resolved, through its deepest existing
/// ancestor when the path itself doesn't exist yet.
fn resolved_path(path: &Path) -> PathBuf {
    let mut rest = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(real) = existing.canonicalize() {
            return rest
                .iter()
                .rev()
                .fold(real, |acc: PathBuf, part| acc.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
}

const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Read-anywhere, write-only-where-allowed profile. Writable roots are passed as `-D`
/// parameters so paths never need escaping inside the profile.
const SEATBELT_BASE: &str = r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow file-read*)
(allow file-write-data
  (require-all (vnode-type CHARACTER-DEVICE)
    (require-any (path "/dev/null") (path "/dev/zero") (path "/dev/dtracehelper") (path "/dev/tty"))))
(allow file-ioctl (path "/dev/tty") (regex #"^/dev/ttys[0-9]+$"))
(allow pseudo-tty)
(allow sysctl-read)
(allow mach-lookup)
(allow ipc-posix-sem)
(allow ipc-posix-shm-read* ipc-posix-shm-write-create ipc-posix-shm-write-data)
(allow iokit-open (iokit-registry-entry-class "RootDomainUserClient"))
(allow user-preference-read)
"#;

const SEATBELT_NETWORK: &str = r#"(allow network-outbound)
(allow network-inbound)
(allow system-socket)
(allow network-bind (local ip "localhost:*"))
"#;

// Preview-only: IOSurface is needed to paint WebKit and Chromium windows (without it
// Chromium cannot allocate its backing surfaces and Tauri's paint smoke never completes).
// Chromium/Electron register this bundle-id + PID service to give ports to their helpers.
// Restrict registration to dev/test identities: a wildcard could squat a production app's
// rendezvous name. Custom packaged apps must use a dev identity or Full access.
// Chromium's single-instance socket binds only inside this preview's private temp folder.
// Do not permit arbitrary Mach registration, GPU clients, shared caches or sandbox extensions.
const SEATBELT_GUI: &str = r#"(allow iokit-open (iokit-user-client-class "IOSurfaceRootUserClient"))
(allow mach-register
  (global-name-regex #"^(ai\.brigadier\.dev|com\.github\.Electron|org\.chromium\.Chromium|com\.google\.chrome\.for\.testing)\.MachPortRendezvousServer\.[0-9]+$"))
(allow network-bind (local unix-socket (subpath (param "PREVIEW_TMP"))))
"#;

impl Seatbelt {
    fn profile(policy: &SandboxPolicy) -> String {
        let mut profile = String::from(SEATBELT_BASE);
        if !policy.writable_roots.is_empty() {
            profile.push_str("(allow file-write*");
            for index in 0..policy.writable_roots.len() {
                profile.push_str(&format!(" (subpath (param \"WRITABLE_ROOT_{index}\"))"));
            }
            profile.push_str(")\n");
        }
        if policy.network {
            profile.push_str(SEATBELT_NETWORK);
        }
        // Later rules win, so these take back reads the base profile allowed.
        if !policy.deny_read.is_empty() {
            profile.push_str("(deny file-read*");
            for index in 0..policy.deny_read.len() {
                profile.push_str(&format!(" (subpath (param \"DENY_READ_{index}\"))"));
            }
            profile.push_str(")\n");
        }
        profile
    }
}

impl Seatbelt {
    fn wrap(
        spec: SpawnSpec,
        policy: &SandboxPolicy,
        preview_temp: Option<&Path>,
        unix_sockets: &[PathBuf],
    ) -> Result<SpawnSpec> {
        let mut profile = Self::profile(policy);
        if preview_temp.is_some() {
            profile.push_str(SEATBELT_GUI);
        }
        for index in 0..unix_sockets.len() {
            profile.push_str(&format!(
                "(allow network-outbound (remote unix-socket (path-literal (param \"UNIX_SOCKET_{index}\"))))\n"
            ));
        }
        let mut args: Vec<OsString> = vec!["-p".into(), profile.into()];
        for (index, root) in policy.writable_roots.iter().enumerate() {
            // Seatbelt matches resolved paths, so symlinks such as /tmp must be resolved first.
            let root = if preview_temp.is_some() {
                // Scoped roots may not exist yet. Resolve existing ancestors without
                // granting the parent directory any extra write access.
                resolved_path(root)
            } else {
                root.canonicalize()?
            };
            let mut define = OsString::from(format!("WRITABLE_ROOT_{index}="));
            define.push(root.as_os_str());
            args.push("-D".into());
            args.push(define);
        }
        for (index, path) in policy.deny_read.iter().enumerate() {
            let path = resolved_path(path);
            let mut define = OsString::from(format!("DENY_READ_{index}="));
            define.push(path.as_os_str());
            args.push("-D".into());
            args.push(define);
        }
        for (index, path) in unix_sockets.iter().enumerate() {
            let mut define = OsString::from(format!("UNIX_SOCKET_{index}="));
            define.push(resolved_path(path).as_os_str());
            args.push("-D".into());
            args.push(define);
        }
        if let Some(temp) = preview_temp {
            let mut define = OsString::from("PREVIEW_TMP=");
            define.push(resolved_path(temp).as_os_str());
            args.push("-D".into());
            args.push(define);
        }
        args.push("--".into());
        args.push(spec.program.into_os_string());
        args.extend(spec.args);
        Ok(SpawnSpec {
            program: PathBuf::from(SANDBOX_EXEC),
            args,
            env: spec.env,
            clear_env: spec.clear_env,
            cwd: spec.cwd,
            low_priority: spec.low_priority,
        })
    }
}

impl Sandbox for Seatbelt {
    fn confine(&self, spec: SpawnSpec, policy: &SandboxPolicy) -> Result<SpawnSpec> {
        Self::wrap(spec, policy, None, &[])
    }

    fn confine_preview(
        &self,
        spec: SpawnSpec,
        policy: &SandboxPolicy,
        unix_sockets: &[PathBuf],
        preview_temp: &Path,
    ) -> Result<SpawnSpec> {
        Self::wrap(spec, policy, Some(preview_temp), unix_sockets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn seatbelt_denies_reading_a_denied_folder_and_reads_the_rest() {
        let dir = Temp(std::env::temp_dir().join(format!("brig-deny-read-{}", std::process::id())));
        let secret = dir.join("run");
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("ipc.token"), "secret").unwrap();
        std::fs::write(dir.join("open.txt"), "open").unwrap();
        let read = |name: &str| {
            let spec = SpawnSpec {
                program: PathBuf::from("/bin/cat"),
                args: vec![dir.join(name).into_os_string()],
                ..SpawnSpec::default()
            };
            let policy = SandboxPolicy {
                deny_read: vec![secret.clone()],
                ..SandboxPolicy::default()
            };
            let spec = Seatbelt.confine(spec, &policy).unwrap();
            Command::new(&spec.program)
                .args(&spec.args)
                .output()
                .unwrap()
        };
        let open = read("open.txt");
        assert!(open.status.success(), "{open:?}");
        assert_eq!(open.stdout, b"open");
        let denied = read("run/ipc.token");
        assert!(!denied.status.success(), "{denied:?}");
        assert!(denied.stdout.is_empty());
    }

    #[test]
    fn gui_preview_keeps_filesystem_and_network_confinement() {
        use std::net::TcpListener;
        use std::os::unix::net::UnixListener;

        // Re-execute this test inside the generated profile to bind a real Unix socket.
        const SOCKET_PROBE: &str = "BRIGADIER_PREVIEW_SOCKET_PROBE";
        if let Some(path) = std::env::var_os(SOCKET_PROBE) {
            let _listener = UnixListener::bind(path).unwrap();
            return;
        }
        let dir =
            Temp(std::env::temp_dir().join(format!("brig-preview-policy-{}", std::process::id())));
        let writable = dir.join("writable");
        let denied = writable.join("secret");
        std::fs::create_dir_all(&denied).unwrap();
        std::fs::write(denied.join("token"), "secret").unwrap();
        std::fs::write(dir.join("readable"), "public").unwrap();
        let other_writable = dir.join("other-writable");
        std::fs::create_dir(&other_writable).unwrap();
        let socket_path = dir.join("allowed.sock");
        let other_path = dir.join("other.sock");
        let allowed_socket = UnixListener::bind(&socket_path).unwrap();
        let other_socket = UnixListener::bind(&other_path).unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut policy = SandboxPolicy {
            writable_roots: vec![writable.clone(), other_writable.clone()],
            deny_read: vec![denied],
            network: false,
        };
        let socket_probe = Seatbelt
            .confine_preview(
                SpawnSpec {
                    program: std::env::current_exe().unwrap(),
                    args: vec![
                        "--exact".into(),
                        "macos::tests::gui_preview_keeps_filesystem_and_network_confinement".into(),
                    ],
                    ..SpawnSpec::default()
                },
                &policy,
                &[],
                &writable,
            )
            .unwrap();
        let bound = Command::new(&socket_probe.program)
            .args(&socket_probe.args)
            .env(SOCKET_PROBE, writable.join("bound.sock"))
            .output()
            .unwrap();
        assert!(bound.status.success(), "{bound:?}");
        assert!(writable.join("bound.sock").exists());
        // A second writable root does not gain permission to bind a socket.
        let denied_bind = Command::new(&socket_probe.program)
            .args(&socket_probe.args)
            .env(SOCKET_PROBE, other_writable.join("bound.sock"))
            .output()
            .unwrap();
        assert!(!denied_bind.status.success(), "{denied_bind:?}");
        let run = |program: &str, args: Vec<OsString>, policy: &SandboxPolicy| {
            let spec = Seatbelt
                .confine_preview(
                    SpawnSpec {
                        program: program.into(),
                        args,
                        ..SpawnSpec::default()
                    },
                    policy,
                    std::slice::from_ref(&socket_path),
                    &writable,
                )
                .unwrap();
            Command::new(spec.program).args(spec.args).output().unwrap()
        };
        let shell = |command: &str, path: &Path| {
            run(
                "/bin/sh",
                vec!["-c".into(), command.into(), "probe".into(), path.into()],
                &policy,
            )
        };
        assert!(
            shell("printf ok > \"$1\"", &writable.join("ok"))
                .status
                .success()
        );
        assert!(
            !shell("printf bad > \"$1\"", &dir.join("outside"))
                .status
                .success()
        );
        assert!(
            !shell("cat \"$1\"", &writable.join("secret/token"))
                .status
                .success()
        );
        assert!(shell("cat \"$1\"", &dir.join("readable")).status.success());
        allowed_socket.set_nonblocking(true).unwrap();
        other_socket.set_nonblocking(true).unwrap();
        let connect = |path: &Path| {
            run(
                "/usr/bin/curl",
                vec![
                    "--unix-socket".into(),
                    path.into(),
                    "--max-time".into(),
                    "1".into(),
                    "--noproxy".into(),
                    "*".into(),
                    "http://localhost/".into(),
                ],
                &policy,
            )
        };
        let connected = connect(&socket_path);
        assert!(allowed_socket.accept().is_ok(), "{connected:?}");
        let _ = connect(&other_path);
        assert!(other_socket.accept().is_err());
        let args = vec![
            "-z".into(),
            "-w".into(),
            "1".into(),
            "127.0.0.1".into(),
            tcp.local_addr().unwrap().port().to_string().into(),
        ];
        assert!(!run("/usr/bin/nc", args.clone(), &policy).status.success());
        policy.network = true;
        assert!(run("/usr/bin/nc", args, &policy).status.success());

        let spec = Seatbelt
            .confine_preview(SpawnSpec::new("/bin/true"), &policy, &[], &writable)
            .unwrap();
        let profile = spec.args[1].to_str().unwrap();
        assert!(profile.contains(SEATBELT_GUI));
        assert!(profile.contains("(allow mach-lookup)"));
        assert!(profile.contains(SEATBELT_NETWORK));
        assert!(profile.contains("(subpath (param \"WRITABLE_ROOT_0\"))"));
        assert!(profile.contains("(deny file-read* (subpath (param \"DENY_READ_0\")))"));
        let ordinary = Seatbelt
            .confine(SpawnSpec::new("/bin/true"), &policy)
            .unwrap();
        assert!(!ordinary.args[1].to_str().unwrap().contains(SEATBELT_GUI));

        // New preview data roots work, but neither their parent nor symlink targets become
        // writable. Denied reads above still override the base read-anywhere rule.
        let fresh = dir.join("not-created-yet");
        policy.writable_roots = vec![fresh.clone()];
        assert!(
            run("/bin/mkdir", vec![fresh.into()], &policy)
                .status
                .success()
        );
        std::os::unix::fs::symlink(&dir.0, writable.join("escape")).unwrap();
        policy.writable_roots = vec![writable.clone()];
        assert!(
            !run(
                "/usr/bin/touch",
                vec![writable.join("escape/escaped").into()],
                &policy
            )
            .status
            .success()
        );
    }

    #[test]
    fn procargs_give_the_arguments_after_the_executable_path() {
        let mut buffer = 2i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(b"/usr/bin/cargo\0\0\0\0cargo\0test\0HOME=/Users/x\0");
        assert_eq!(
            parse_procargs(&buffer),
            Some(vec!["cargo".to_owned(), "test".to_owned()])
        );
        assert_eq!(parse_procargs(&[1, 0]), None);
    }

    #[test]
    fn reads_a_child_s_arguments_and_stops_and_continues_it() {
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        assert_eq!(
            MacProcesses.command_line(pid),
            Some(vec!["/bin/sleep".to_owned(), "30".to_owned()])
        );
        let state = || {
            let out = Command::new("ps")
                .args(["-o", "stat=", "-p", &pid.to_string()])
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        MacProcesses.suspend(pid).unwrap();
        assert!(state().starts_with('T'), "stopped: {}", state());
        MacProcesses.resume(pid).unwrap();
        assert!(!state().starts_with('T'), "running: {}", state());
        assert!(MacProcesses.cpu_time_ms(pid).is_some());
        assert!(MacProcesses.cpu_time_ms(std::process::id()).is_some());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn start_times_come_from_the_kernel_for_any_process_and_zombies() {
        let now_ms = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as f64
        };
        let own = MacProcesses.start_time_ms(std::process::id()).unwrap();
        assert!(own > now_ms() - 3_600_000.0 && own <= now_ms(), "{own}");
        let before = now_ms();
        let mut child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let after = now_ms();
        let started = MacProcesses.start_time_ms(child.id()).unwrap();
        assert!(
            started >= before - 1_000.0 && started <= after + 1_000.0,
            "{before} <= {started} <= {after}"
        );
        // Another user's process (launchd), which PROC_PIDTBSDINFO refuses.
        assert!(MacProcesses.start_time_ms(1).unwrap() <= own);
        // A zombie keeps its start time until it is reaped.
        child.kill().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !is_zombie(child.id()) {
            assert!(
                std::time::Instant::now() < deadline,
                "never became a zombie"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(MacProcesses.start_time_ms(child.id()).unwrap(), started);
        let pid = child.id();
        child.wait().unwrap();
        assert!(MacProcesses.start_time_ms(pid).is_err());
    }

    #[test]
    fn reads_the_machine_s_load() {
        assert!(thermal_state().is_some());
        assert!(memory_pressure_level().is_some());
        let _ = MacMachine.load();
    }
}
