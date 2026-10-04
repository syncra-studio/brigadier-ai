//! Windows: everything the daemon needs to run, with current-user-only ACLs on private files.
//! The worker sandbox, credential storage and login-shell resolution arrive with the Windows
//! platform phase.
#![allow(unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, FILETIME, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
    STILL_ACTIVE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, GetCurrentProcess,
    GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
};

use crate::machine::memory_tight_from_load;
use crate::{
    AppPaths, CredentialStore, DetachedChild, Machine, MachineLoad, Platform, PrivateFs, Processes,
    Result, Sandbox, SandboxPolicy, Shell, SpawnSpec, unsupported,
};

const NAME: &str = "windows";

pub(crate) struct Windows {
    paths: AppPaths,
}

impl Windows {
    pub(crate) fn new(paths: AppPaths) -> Self {
        Self { paths }
    }
}

impl Platform for Windows {
    fn name(&self) -> &'static str {
        NAME
    }
    fn paths(&self) -> &AppPaths {
        &self.paths
    }
    fn private_fs(&self) -> &dyn PrivateFs {
        &WindowsPrivateFs
    }
    fn processes(&self) -> &dyn Processes {
        &WindowsProcesses
    }
    fn credentials(&self) -> &dyn CredentialStore {
        &Unsupported
    }
    fn shell(&self) -> &dyn Shell {
        &Unsupported
    }
    fn sandbox(&self) -> &dyn Sandbox {
        &Unsupported
    }
    fn machine(&self) -> &dyn Machine {
        &WindowsMachine
    }
}

/// The memory load. Windows has no reliable unprivileged temperature or heat-state API, so
/// heat reads as nominal; a CPU-throttling signal arrives with the Windows platform phase.
struct WindowsMachine;

impl Machine for WindowsMachine {
    fn load(&self) -> MachineLoad {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        // SAFETY: `status` is a live MEMORYSTATUSEX with `dwLength` set, as the call requires.
        let read = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
        MachineLoad {
            heat: crate::Heat::Nominal,
            memory_tight: read && memory_tight_from_load(status.dwMemoryLoad),
        }
    }
}

/// SDDL granting full access to the current user only, with inheritance disabled (`P`).
pub fn current_user_only_sddl(inheritable: bool) -> io::Result<String> {
    let sid = current_user_sid()?;
    let flags = if inheritable { "OICI" } else { "" };
    Ok(format!("D:P(A;{flags};GA;;;{sid})"))
}

/// A security descriptor built from SDDL, freed on drop.
pub struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    pub fn from_sddl(sddl: &str) -> io::Result<Self> {
        let wide = wide(OsStr::new(sddl));
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `descriptor` receives a
        // LocalAlloc'd pointer that we own and free in Drop.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }

    pub fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0
    }

    fn dacl(&self) -> io::Result<*mut ACL> {
        let mut present = 0;
        let mut defaulted = 0;
        let mut dacl: *mut ACL = ptr::null_mut();
        // SAFETY: self.0 is a valid descriptor for the lifetime of self.
        let ok =
            unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) };
        if ok == 0 || present == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(dacl)
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.0.cast()) };
    }
}

// SAFETY: the descriptor is immutable after construction and owned exclusively.
unsafe impl Send for SecurityDescriptor {}
unsafe impl Sync for SecurityDescriptor {}

fn current_user_sid() -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentProcess returns a pseudo-handle; `token` is closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let result = (|| {
        let mut len = 0u32;
        // SAFETY: a null buffer with length 0 asks for the required size.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len) };
        let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
        // SAFETY: `buffer` is at least `len` bytes and 8-byte aligned, enough for TOKEN_USER.
        if unsafe {
            GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), len, &mut len)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetTokenInformation(TokenUser) wrote a TOKEN_USER at the start of `buffer`.
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let mut sid_string: *mut u16 = ptr::null_mut();
        // SAFETY: the SID lives inside `buffer`; `sid_string` is LocalAlloc'd and freed below.
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_string) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `sid_string` is a NUL-terminated wide string from the call above.
        let sid = unsafe {
            let len = (0..).take_while(|&i| *sid_string.add(i) != 0).count();
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(sid_string, len));
            LocalFree(sid_string.cast());
            sid
        };
        Ok(sid)
    })();
    // SAFETY: `token` was opened above.
    unsafe { CloseHandle(token) };
    result
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

struct WindowsPrivateFs;

impl PrivateFs for WindowsPrivateFs {
    fn create_private_dir(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let descriptor = SecurityDescriptor::from_sddl(&current_user_only_sddl(true)?)?;
        let dacl = descriptor.dacl()?;
        let path = wide(dir.as_os_str());
        // SAFETY: `path` is NUL-terminated; `dacl` points into `descriptor`, alive for the call.
        let status = unsafe {
            SetNamedSecurityInfoW(
                path.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                dacl,
                ptr::null(),
            )
        };
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32).into());
        }
        Ok(())
    }

    fn create_private_file(&self, path: &Path) -> Result<File> {
        let descriptor = SecurityDescriptor::from_sddl(&current_user_only_sddl(false)?)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.as_ptr(),
            bInheritHandle: 0,
        };
        let name = wide(path.as_os_str());
        // SAFETY: all pointers are valid for the duration of the call; CREATE_NEW makes the file
        // private from creation and fails if it already exists.
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().into());
        }
        // SAFETY: `handle` is a freshly opened file handle that the File now owns.
        Ok(unsafe { File::from_raw_handle(handle as RawHandle) })
    }
}

struct WindowsProcesses;

/// Owned process handle.
struct ProcessHandle(HANDLE);

impl ProcessHandle {
    fn open(pid: u32, access: u32) -> io::Result<Self> {
        // SAFETY: plain FFI call; a null result is checked.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by OpenProcess and is closed exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

impl Processes for WindowsProcesses {
    fn spawn_detached(&self, spec: &SpawnSpec) -> Result<DetachedChild> {
        let spawn = |flags: u32| -> io::Result<std::process::Child> {
            let mut command: Command = spec.command();
            command.creation_flags(flags);
            command.spawn()
        };
        let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        // Break away from the caller's job object when allowed so closing the app does not
        // take the daemon down; fall back when the job forbids breakaway.
        let child = match spawn(base | CREATE_BREAKAWAY_FROM_JOB) {
            Ok(child) => child,
            Err(_) => spawn(base)?,
        };
        Ok(DetachedChild::new(child))
    }

    fn piped_command(&self, spec: &SpawnSpec) -> Command {
        let mut command = spec.piped();
        let priority = if spec.low_priority {
            windows_sys::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS
        } else {
            0
        };
        command.creation_flags(
            CREATE_NEW_PROCESS_GROUP
                | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                | priority,
        );
        command
    }

    fn is_alive(&self, pid: u32) -> bool {
        let Ok(handle) = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
            return false;
        };
        let mut code = 0u32;
        // SAFETY: `handle` is valid; `code` is a valid out pointer.
        let ok = unsafe { GetExitCodeProcess(handle.0, &mut code) };
        ok != 0 && code == STILL_ACTIVE as u32
    }

    fn terminate(&self, pid: u32) -> Result<()> {
        let handle = ProcessHandle::open(pid, PROCESS_TERMINATE)?;
        // SAFETY: `handle` was opened with PROCESS_TERMINATE.
        if unsafe { TerminateProcess(handle.0, 1) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    fn kill_group(&self, _pid: u32) -> Result<()> {
        // Windows process groups only route console control events; nothing can be signalled
        // through one once its leader is gone. `kill_tree` covers a live process's tree.
        Ok(())
    }

    fn kill_tree(&self, pid: u32) -> Result<()> {
        let status = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .output()?
            .status;
        if status.success() || !self.is_alive(pid) {
            Ok(())
        } else {
            Err(io::Error::other(format!("taskkill exited with {status}")).into())
        }
    }

    fn descendants(&self, _pid: u32) -> Result<Vec<u32>> {
        // Walking the tree needs a Toolhelp snapshot; this arrives with the Windows platform
        // phase. `kill_tree` (`taskkill /T`) still ends a live CLI's whole tree.
        unsupported("listing a process's descendants", NAME)
    }

    fn children(&self, _pid: u32) -> Result<Vec<u32>> {
        // As `descendants`.
        unsupported("listing a process's children", NAME)
    }

    fn group_of(&self, _pid: u32) -> Option<u32> {
        // Windows has no process groups to signal (see `kill_group`).
        None
    }

    fn command_line(&self, _pid: u32) -> Option<Vec<String>> {
        // Reading another process's command line needs its PEB, as `in_dir`.
        None
    }

    fn suspend(&self, _pid: u32) -> Result<()> {
        // Suspending a process (NtSuspendProcess or a job object) arrives with the Windows
        // platform phase, together with the tree walk that finds what to suspend.
        unsupported("suspending a process", NAME)
    }

    fn resume(&self, _pid: u32) -> Result<()> {
        unsupported("resuming a process", NAME)
    }

    fn in_dir(&self, _dir: &std::path::Path) -> Result<Vec<u32>> {
        // Reading another process's working directory needs its PEB; this arrives with the
        // Windows platform phase. `kill_tree` still ends a CLI's whole tree.
        unsupported("finding processes by working directory", NAME)
    }

    fn start_time_ms(&self, pid: u32) -> Result<f64> {
        let handle = ProcessHandle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        // SAFETY: `handle` is valid and every out pointer refers to a live FILETIME.
        if unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
            == 0
        {
            return Err(io::Error::last_os_error().into());
        }
        // FILETIME counts 100 ns intervals since 1601-01-01.
        const EPOCH_DIFFERENCE_100NS: u64 = 116_444_736_000_000_000;
        let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        Ok(ticks.saturating_sub(EPOCH_DIFFERENCE_100NS) as f64 / 10_000.0)
    }
}

struct Unsupported;

impl CredentialStore for Unsupported {
    fn set(&self, _account: &str, _secret: &[u8]) -> Result<()> {
        unsupported("credential storage", NAME)
    }
    fn get(&self, _account: &str) -> Result<Option<Vec<u8>>> {
        unsupported("credential storage", NAME)
    }
    fn delete(&self, _account: &str) -> Result<()> {
        unsupported("credential storage", NAME)
    }
}

impl Shell for Unsupported {
    fn login_shell(&self) -> Result<PathBuf> {
        unsupported("login shell resolution", NAME)
    }
    fn login_environment(&self) -> Result<BTreeMap<OsString, OsString>> {
        unsupported("login shell resolution", NAME)
    }
}

impl Sandbox for Unsupported {
    fn confine(&self, _spec: SpawnSpec, _policy: &SandboxPolicy) -> Result<SpawnSpec> {
        unsupported("the worker sandbox", NAME)
    }
}
