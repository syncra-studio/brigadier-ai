//! Unix-socket daemon server and client for `brigadier-cu serve`/`stop`/`status`.
//!
//! Protocol: line-delimited JSON over a Unix domain socket.
//!
//! Request shapes:
//!   {"method":"call","name":"<tool>","args":{...}}
//!   {"method":"list"}
//!   {"method":"describe","name":"<tool>"}
//!   {"method":"shutdown"}
//!   {"method":"permissions_status"}       (private macOS operator control)
//!   {"method":"permissions_present"}      (private macOS operator control)
//!   {"method":"permissions_refresh"}      (private macOS operator control)
//!   {"method":"application_windows"}      (private authenticated Brigadier host control)
//!   {"method":"application_surface_start","args":{...}}
//!   {"method":"application_surface_stop","args":{"session":"..."}}
//!   {"method":"application_surface_event","args":{...}}
//!   {"method":"reassert_cursor","args":{"session":"...","window_id":123}}
//!
//! Response shapes:
//!   {"ok":true,"result":...}
//!   {"ok":false,"error":"...","exit_code":1}
//!
//! The socket file is at:
//!   macOS  — <home>/cu.sock (see `pip_preview::home`)
//!   Linux  — ~/.cache/brigadier-cu/brigadier-cu.sock
//!   Windows — \\.\pipe\brigadier-cu  (TODO: use named pipe; stubs only for now)

use base64::Engine as _;
use serde::{Deserialize, Serialize};

/// Why `run_serve_with_owner` returned without an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(not(unix), allow(dead_code))]
pub enum ServeExit {
    /// A client sent `shutdown`, or the authorized root process exited.
    Shutdown,
    /// The `--owner-pid` process exited (or was already dead at start).
    OwnerExited,
}

/// Tool surface owned by a daemon instance. The proxy validates this value
/// before exposing any tools, so a client never bridges to a daemon serving a
/// different surface.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DaemonProfile {
    #[serde(rename = "native")]
    Native,
}

impl DaemonProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
        }
    }
}

impl std::fmt::Display for DaemonProfile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(unix)]
fn secure_runtime_directory(
    dir: &std::path::Path,
    repair_legacy_permissions: bool,
) -> anyhow::Result<()> {
    open_private_runtime_directory(dir, repair_legacy_permissions).map(drop)
}

#[cfg(unix)]
fn open_directory_component(
    parent: std::os::fd::BorrowedFd<'_>,
    name: &std::ffi::OsStr,
    display: &std::path::Path,
) -> anyhow::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let name = std::ffi::CString::new(name.as_bytes())
        .map_err(|_| anyhow::anyhow!("runtime path contains a NUL byte: {}", display.display()))?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ELOOP) | Some(libc::ENOTDIR) => {
                anyhow::bail!(
                    "runtime path is not a real directory: {}",
                    display.display()
                )
            }
            _ => Err(anyhow::anyhow!("open {}: {error}", display.display())),
        };
    }
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
}

#[cfg(unix)]
fn directory_status(
    directory: std::os::fd::BorrowedFd<'_>,
    display: &std::path::Path,
) -> anyhow::Result<libc::stat> {
    use std::os::fd::AsRawFd;

    let mut status = std::mem::MaybeUninit::<libc::stat>::zeroed();
    if unsafe { libc::fstat(directory.as_raw_fd(), status.as_mut_ptr()) } != 0 {
        let error = std::io::Error::last_os_error();
        anyhow::bail!("stat {}: {error}", display.display());
    }
    let status = unsafe { status.assume_init() };
    if status.st_mode & libc::S_IFMT != libc::S_IFDIR {
        anyhow::bail!(
            "runtime path is not a real directory: {}",
            display.display()
        );
    }
    Ok(status)
}

/// An ancestor may be owned by root, by us, or (inside a non-initial Linux
/// user namespace only) by the overflow uid of an unmapped owner.
#[cfg(unix)]
fn ancestor_owner_is_trusted(
    owner: libc::uid_t,
    effective_uid: libc::uid_t,
    unmapped_owner: Option<libc::uid_t>,
) -> bool {
    owner == 0 || owner == effective_uid || Some(owner) == unmapped_owner
}

/// An ancestor of the runtime directory must not let another user rename or
/// replace the entry below it. It must be owned by root or by us, and it must
/// not be writable by group or other users unless the sticky bit restricts
/// renames to the entry owner (as on `/tmp`).
#[cfg(unix)]
fn check_runtime_ancestor(
    directory: std::os::fd::BorrowedFd<'_>,
    display: &std::path::Path,
    effective_uid: libc::uid_t,
) -> anyhow::Result<()> {
    let status = directory_status(directory, display)?;
    if !ancestor_owner_is_trusted(status.st_uid, effective_uid, unmapped_owner_uid()) {
        anyhow::bail!(
            "runtime path ancestor {} is owned by uid {}, expected root or uid {}",
            display.display(),
            status.st_uid,
            effective_uid
        );
    }
    let mode = status.st_mode as u32;
    if mode & 0o022 != 0 && mode & (libc::S_ISVTX as u32) == 0 {
        anyhow::bail!(
            "runtime path ancestor {} is writable by other users without the sticky bit mode={:o}",
            display.display(),
            mode & 0o7777
        );
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "linux")))]
fn unmapped_owner_uid() -> Option<libc::uid_t> {
    None
}

/// Open the private runtime directory without following any symlink, and
/// return a descriptor that later socket operations use. Each component is
/// opened relative to the descriptor of its parent, starting at `/`, with
/// `O_NOFOLLOW`. A rename of any path component after this call cannot
/// redirect work done through the returned descriptor.
///
/// The ancestors are resolved once with `canonicalize` so a root-owned system
/// symlink such as `/tmp -> private/tmp` is allowed; every resolved ancestor
/// is then checked by `check_runtime_ancestor`. The final directory itself
/// must not be a symlink, must be owned by us, and must be private.
#[cfg(unix)]
fn open_private_runtime_directory(
    dir: &std::path::Path,
    repair_legacy_permissions: bool,
) -> anyhow::Result<std::os::fd::OwnedFd> {
    use std::os::fd::{AsFd, AsRawFd, FromRawFd};
    use std::os::unix::fs::DirBuilderExt;

    let dir = if dir.as_os_str().is_empty() {
        std::env::current_dir()?
    } else if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()?.join(dir)
    };

    if !dir.exists() {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(&dir)?;
    }

    let name = match dir.components().next_back() {
        Some(std::path::Component::Normal(name)) => name.to_owned(),
        _ => anyhow::bail!("runtime path has no directory name: {}", dir.display()),
    };
    let parent = dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("runtime path has no parent: {}", dir.display()))?;
    let parent = std::fs::canonicalize(parent)?;

    let effective_uid = unsafe { libc::geteuid() };
    let root_path = std::path::Path::new("/");
    let root_fd = unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        let error = std::io::Error::last_os_error();
        anyhow::bail!("open /: {error}");
    }
    let mut current = unsafe { std::os::fd::OwnedFd::from_raw_fd(root_fd) };
    check_runtime_ancestor(current.as_fd(), root_path, effective_uid)?;

    let mut current_path = root_path.to_path_buf();
    for component in parent.components() {
        match component {
            std::path::Component::RootDir => continue,
            std::path::Component::Normal(part) => {
                current_path.push(part);
                let next = open_directory_component(current.as_fd(), part, &current_path)?;
                check_runtime_ancestor(next.as_fd(), &current_path, effective_uid)?;
                current = next;
            }
            _ => anyhow::bail!(
                "runtime path ancestor is not canonical: {}",
                parent.display()
            ),
        }
    }

    let directory = open_directory_component(current.as_fd(), &name, &dir)?;
    let status = directory_status(directory.as_fd(), &dir)?;
    if status.st_uid != effective_uid {
        anyhow::bail!(
            "runtime directory {} is owned by uid {}, expected {}",
            dir.display(),
            status.st_uid,
            effective_uid
        );
    }

    let mode = status.st_mode as u32 & 0o777;
    if mode & 0o077 != 0 {
        if !repair_legacy_permissions {
            anyhow::bail!(
                "runtime directory has unsafe permissions path={} mode={mode:o}",
                dir.display()
            );
        }
        if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0 {
            let error = std::io::Error::last_os_error();
            anyhow::bail!("chmod {}: {error}", dir.display());
        }
    }
    Ok(directory)
}

/// Give the calling thread its own working directory, set to `directory`.
/// The process working directory and every other thread are unchanged. The
/// caller must be a short-lived thread that exits after its relative-path
/// work, which discards the per-thread directory.
#[cfg(target_os = "macos")]
fn set_thread_working_directory(directory: std::os::fd::BorrowedFd<'_>) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    extern "C" {
        // Exported by libsystem_pthread since macOS 10.12.
        fn pthread_fchdir_np(fd: libc::c_int) -> libc::c_int;
    }
    if unsafe { pthread_fchdir_np(directory.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Bind `name` inside the already opened and checked `directory`. The stale
/// socket check, the bind, and the chmod all run on a dedicated thread whose
/// working directory is `directory`, and they use the bare relative `name`.
/// The kernel therefore resolves the socket inside the checked directory even
/// if a component of the original path was renamed after the check.
#[cfg(unix)]
fn bind_socket_in_directory(
    directory: &std::os::fd::OwnedFd,
    name: &std::ffi::OsStr,
    display: &std::path::Path,
) -> anyhow::Result<std::os::unix::net::UnixListener> {
    use std::os::fd::AsFd;

    let relative = std::path::Path::new(name);
    if relative.components().count() != 1
        || !matches!(
            relative.components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        anyhow::bail!(
            "socket name is not a single path component: {}",
            display.display()
        );
    }
    std::thread::scope(|scope| {
        scope
            .spawn(|| -> anyhow::Result<std::os::unix::net::UnixListener> {
                set_thread_working_directory(directory.as_fd()).map_err(|error| {
                    anyhow::anyhow!("enter socket directory for {}: {error}", display.display())
                })?;
                remove_stale_socket(relative, display)?;
                let listener = std::os::unix::net::UnixListener::bind(relative)
                    .map_err(|error| anyhow::anyhow!("bind {}: {error}", display.display()))?;
                set_private_socket_permissions(relative)?;
                Ok(listener)
            })
            .join()
            .map_err(|_| anyhow::anyhow!("socket bind thread panicked for {}", display.display()))?
    })
}

#[cfg(unix)]
fn remove_stale_socket(
    socket_path: &std::path::Path,
    display: &std::path::Path,
) -> anyhow::Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };

    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        anyhow::bail!("refusing to replace non-socket path: {}", display.display());
    }

    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        anyhow::bail!(
            "socket {} is owned by uid {}, expected {}",
            display.display(),
            metadata.uid(),
            effective_uid
        );
    }

    if std::os::unix::net::UnixStream::connect(socket_path).is_ok() {
        anyhow::bail!("daemon is already listening on {}", display.display());
    }

    std::fs::remove_file(socket_path)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_socket_permissions(socket_path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(unix)]
fn write_private_pid_file(pid_path: &std::path::Path) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(pid_path)?;
    file.write_all(std::process::id().to_string().as_bytes())?;
    file.sync_all()?;
    std::fs::set_permissions(pid_path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

/// Refuse a daemon socket peer that runs as a different user.
///
/// The socket directory and socket file are private, but that is a path
/// property. The kernel peer credential is the authoritative identity of a
/// connected process, so every accepted connection must carry our effective
/// uid. The returned message names only uids, never request data or tokens.
#[cfg(unix)]
fn check_peer_uid(stream: &tokio::net::UnixStream, expected_uid: u32) -> Result<(), String> {
    let credentials = stream
        .peer_cred()
        .map_err(|error| format!("could not read the peer credentials: {error}"))?;
    let peer_uid = credentials.uid();
    if peer_uid == expected_uid {
        Ok(())
    } else {
        Err(format!(
            "peer runs as uid {peer_uid}, expected uid {expected_uid}"
        ))
    }
}

#[cfg(all(unix, target_os = "macos"))]
type SecurityCodeRef = *const libc::c_void;
#[cfg(all(unix, target_os = "macos"))]
type SecurityRequirementRef = *const libc::c_void;
#[cfg(all(unix, target_os = "macos"))]
type SecurityStatus = i32;
#[cfg(all(unix, target_os = "macos"))]
type SecurityFlags = u32;

#[cfg(all(unix, target_os = "macos"))]
#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecGuestAttributePid: core_foundation::string::CFStringRef;
    fn SecCodeCopySelf(flags: SecurityFlags, code: *mut SecurityCodeRef) -> SecurityStatus;
    fn SecCodeCopyDesignatedRequirement(
        code: SecurityCodeRef,
        flags: SecurityFlags,
        requirement: *mut SecurityRequirementRef,
    ) -> SecurityStatus;
    fn SecCodeCopyGuestWithAttributes(
        host: SecurityCodeRef,
        attributes: core_foundation::dictionary::CFDictionaryRef,
        flags: SecurityFlags,
        guest: *mut SecurityCodeRef,
    ) -> SecurityStatus;
    fn SecCodeCheckValidity(
        code: SecurityCodeRef,
        flags: SecurityFlags,
        requirement: SecurityRequirementRef,
    ) -> SecurityStatus;
}

#[cfg(all(unix, target_os = "macos"))]
fn dynamic_code_for_pid(pid: libc::pid_t, label: &str) -> Result<SecurityCodeRef, String> {
    use core_foundation::base::TCFType;
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;

    let pid_key = unsafe { CFString::wrap_under_get_rule(kSecGuestAttributePid) };
    let attributes = CFDictionary::from_CFType_pairs(&[(pid_key, CFNumber::from(pid))]);
    let mut code: SecurityCodeRef = std::ptr::null();
    let status = unsafe {
        SecCodeCopyGuestWithAttributes(
            std::ptr::null(),
            attributes.as_concrete_TypeRef(),
            0,
            &mut code,
        )
    };
    if status == 0 {
        Ok(code)
    } else {
        Err(format!(
            "Security.framework could not inspect {label} pid {pid} (OSStatus {status})"
        ))
    }
}

#[cfg(all(unix, target_os = "macos"))]
fn validate_peer_matches_daemon_code(peer_pid: libc::pid_t) -> Result<(), String> {
    use core_foundation::base::{CFRelease, CFTypeRef};

    let mut daemon_code: SecurityCodeRef = std::ptr::null();
    let self_status = unsafe { SecCodeCopySelf(0, &mut daemon_code) };
    if self_status != 0 {
        return Err(format!(
            "Security.framework could not inspect the running daemon (OSStatus {self_status})"
        ));
    }

    let mut daemon_requirement: SecurityRequirementRef = std::ptr::null();
    let requirement_status =
        unsafe { SecCodeCopyDesignatedRequirement(daemon_code, 0, &mut daemon_requirement) };
    if requirement_status != 0 {
        unsafe { CFRelease(daemon_code as CFTypeRef) };
        return Err(format!(
            "Security.framework could not derive the running daemon code requirement (OSStatus {requirement_status})"
        ));
    }

    let peer_code = match dynamic_code_for_pid(peer_pid, "broker") {
        Ok(code) => code,
        Err(error) => {
            unsafe {
                CFRelease(daemon_requirement as CFTypeRef);
                CFRelease(daemon_code as CFTypeRef);
            }
            return Err(error);
        }
    };
    let validity_status = unsafe { SecCodeCheckValidity(peer_code, 0, daemon_requirement) };
    unsafe {
        CFRelease(peer_code as CFTypeRef);
        CFRelease(daemon_requirement as CFTypeRef);
        CFRelease(daemon_code as CFTypeRef);
    }
    if validity_status == 0 {
        Ok(())
    } else {
        Err(format!(
            "broker pid {peer_pid} does not match the running daemon code identity (OSStatus {validity_status})"
        ))
    }
}

/// Optional shared secret for authenticating daemon socket requests.
///
/// The default standalone driver remains backward-compatible when this is
/// unset. Embedders that expose TCC- or input-capable tools over a predictable
/// local socket set an unguessable value in both the daemon and its trusted
/// proxy processes.
pub const SOCKET_AUTH_TOKEN_ENV: &str = "BRIGADIER_CU_SOCKET_AUTH_TOKEN";

/// Optional second capability reserved for embedding-host operations.
///
/// The ordinary socket token authorizes Computer Use tool calls. Embedders
/// keep this separate token out of agent environments and attach it only to
/// shutdown, permission, and state-authentication requests. When configured,
/// authenticated agents may reconnect after an embedding host relaunch
/// without gaining those host-only capabilities.
pub const SOCKET_HOST_AUTH_TOKEN_ENV: &str = "BRIGADIER_CU_SOCKET_HOST_AUTH_TOKEN";

/// Optional macOS process-ancestry root for daemon socket clients.
///
/// Embedders set this to their own PID. Even if another same-UID process finds
/// the bearer token, the daemon rejects its socket unless the kernel-reported
/// peer PID is the embedder or one of its descendants. Embedders that configure
/// a separate host capability use this identity as a lifecycle owner instead:
/// ordinary authenticated agents may outlive and reconnect across host
/// generations, while the daemon still exits when its current owner exits.
pub const SOCKET_AUTHORIZED_ROOT_PID_ENV: &str = "BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_PID";
pub const SOCKET_AUTHORIZED_ROOT_START_SECONDS_ENV: &str =
    "BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_START_SECONDS";
pub const SOCKET_AUTHORIZED_ROOT_START_MICROSECONDS_ENV: &str =
    "BRIGADIER_CU_SOCKET_AUTHORIZED_ROOT_START_MICROSECONDS";

// ── Recording idle-TTL backstop (#1764) ─────────────────────────────────────────

/// Default TTL of zero `call` activity after which an active recording is
/// auto-stopped. Generous: a single agent turn can be slow. Overridable via the
/// `BRIGADIER_CU_RECORDING_IDLE_TTL_SECS` env var (used by the #1764 verify
/// harness to exercise the backstop quickly).
const RECORDING_IDLE_TTL_SECS_DEFAULT: u64 = 300;

/// Wall-clock seconds since the Unix epoch. Same idiom `recording.rs` uses for
/// `now_ms`.
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Resolve + apply the session identity for a tool call at the daemon boundary.
///
/// A session is a **caller-declared** identity (the public `session` arg), not a
/// property of the MCP connection. We mirror an explicit `session` into the
/// reserved `_session_id` key that every session-aware tool already reads
/// (cursor key, per-session config override, recording owner). When no `session`
/// was declared we fall back to the per-connection minted id for `_session_id`
/// ONLY — that preserves connection-EOF cleanup of recording / config as before.
/// The cursor is deliberately NOT driven by that fallback: `resolve_cursor_key`
/// reads the explicit `session`/`cursor_id` arg only, so a cursor appears
/// exactly when a run declares its session (explicit-required).
///
/// Also refreshes the idle-TTL clock for an explicit session (the minted
/// fallback is reaped by EOF, not TTL). Returns the effective `_session_id` for
/// the resurrection guard.
fn apply_session_identity(args: &mut serde_json::Value, minted: &Option<String>) -> Option<String> {
    let explicit = args
        .as_object()
        .and_then(|o| o.get("session"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned());
    if let Some(obj) = args.as_object_mut() {
        if !obj.contains_key("_session_id") {
            if let Some(id) = explicit.clone().or_else(|| minted.clone()) {
                obj.insert("_session_id".to_owned(), serde_json::Value::String(id));
            }
        }
    }
    if let Some(sess) = &explicit {
        cu_core::session::touch_session(sess);
    }
    args.as_object()
        .and_then(|o| o.get("_session_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
}

/// Whether `tool_name` manages session lifecycle and so must be EXEMPT from the
/// resurrection guard. `start_session` revives an ended id (the explicit,
/// caller-intended way to reuse one) and `end_session` is idempotent — both
/// would be wrongly rejected if the guard gated them on an already-ended id.
fn is_session_lifecycle_tool(tool_name: &str) -> bool {
    matches!(tool_name, "start_session" | "end_session")
}

/// Resolve the recording idle TTL, honoring the env override.
fn recording_idle_ttl_secs() -> u64 {
    std::env::var("BRIGADIER_CU_RECORDING_IDLE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(RECORDING_IDLE_TTL_SECS_DEFAULT)
}

/// Spawn a detached daemon task that auto-stops a recording after the idle TTL.
///
/// Defense-in-depth for #1764: the primary teardown is now the per-session
/// reaper (the proxy's persistent control connection EOF fires `session_end`,
/// covering proxy SIGKILL/crash). This TTL covers the residual case a non-proxy
/// raw client starts a recording and dies without ever sending `session_begin`.
/// It MUST NOT stop a still-active session: it only reaps when BOTH
/// the recording is enabled AND there has been no `call` activity for the full
/// TTL. As long as a session keeps issuing tool calls, `last_activity` is bumped
/// every turn and the idle window never reaches the TTL. `stop()` is idempotent,
/// so racing with the proxy-exit hook or an explicit `stop_recording` is benign.
fn spawn_recording_idle_backstop(
    registry: std::sync::Arc<cu_core::tool::ToolRegistry>,
    last_activity: std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    let ttl = recording_idle_ttl_secs();
    tokio::spawn(async move {
        // 30s tick granularity: reap latency is `ttl` rounded up to the next
        // tick, so a sub-30s TTL override (e.g. in tests) still fires no sooner
        // than ~30s. Fine for the 300s production default.
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            if registry.recording.current_state().enabled {
                let idle = now_unix_secs()
                    .saturating_sub(last_activity.load(std::sync::atomic::Ordering::Relaxed));
                if idle >= ttl {
                    tracing::warn!(
                        "recording idle {idle}s ≥ {ttl}s TTL; auto-stopping \
                         (proxy-exit hook likely missed)"
                    );
                    // Unconditional (`None`): the idle backstop is a last-resort
                    // GLOBAL kill of whatever recording is active after global
                    // inactivity — not an owner reclaiming a specific session.
                    // It already targets the live recording by definition, so a
                    // requester id would be redundant and could only make it
                    // wrongly no-op. Session-scoped teardown is the proxy-exit
                    // `session_end` path (#1764 / session-identity work).
                    // stop_owner can SYNCHRONOUSLY finalize the recording's mp4
                    // (SCStream::stop_capture blocks on disk I/O), so run it on a
                    // blocking thread to keep the reactor free (see the EOF reaper).
                    let reg2 = registry.clone();
                    let _ =
                        tokio::task::spawn_blocking(move || reg2.recording.stop_owner(None)).await;
                }
            }
        }
    });
}

/// Default idle-TTL for a caller-declared session (seconds). A session that
/// isn't touched (no tool call carrying its `session`) for this long is reclaimed
/// by [`spawn_session_idle_sweep`] — its cursor removed, recording stopped,
/// config cleared. Overridable via `BRIGADIER_CU_SESSION_IDLE_TTL_SECS`.
const SESSION_IDLE_TTL_SECS_DEFAULT: u64 = 300;

fn session_idle_ttl_secs() -> u64 {
    std::env::var("BRIGADIER_CU_SESSION_IDLE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(SESSION_IDLE_TTL_SECS_DEFAULT)
}

/// Spawn the detached idle-TTL sweep for caller-declared sessions.
///
/// A session is no longer tied to a connection's lifetime (it's a caller-declared
/// identity that can span connections, transports, and apps), so a run that ends
/// — or crashes — without calling `end_session` is reclaimed here. Each tick ends
/// every session whose last activity is older than the TTL; `session::evict_idle`
/// fans `fire_session_end` out to the cursor/recording/config cleanup hooks. A
/// session that keeps issuing tool calls bumps its activity every turn and never
/// reaches the idle window. Idempotent and cheap.
fn spawn_session_idle_sweep() {
    let ttl = std::time::Duration::from_secs(session_idle_ttl_secs());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            let ended = cu_core::session::evict_idle(ttl);
            if !ended.is_empty() {
                tracing::info!(
                    count = ended.len(),
                    "idle-TTL reclaimed sessions: {ended:?}"
                );
            }
        }
    });
}

/// Register a `session_end` hook that stops a recording the ending session owns.
///
/// The per-platform cursor-remove + config-clear hooks already run on
/// `fire_session_end`; this adds recording teardown to the SAME signal, so
/// `end_session`, the idle-TTL sweep, and the control-connection EOF reaper all
/// stop a session's recording uniformly (matching `end_session`'s contract).
/// `stop_owner(Some(sid))` is a no-op unless `sid` owns the live recording, and
/// runs on a detached thread so finalizing the mp4 never blocks the synchronous
/// `fire_session_end` caller (the sweep task or an async tool invoke). The EOF
/// arm keeps its own inline `spawn_blocking` stop for ordered finalize-then-reply;
/// a second stop here is an idempotent no-op.
fn register_recording_session_end_hook(
    recording: std::sync::Arc<cu_core::recording::RecordingSession>,
) {
    cu_core::session::register_session_end_hook(move |sid| {
        let recording = recording.clone();
        let sid = sid.to_owned();
        std::thread::spawn(move || {
            let _ = recording.stop_owner(Some(&sid));
        });
    });
}

/// Removes per-session activity state when the same lifecycle fan-out tears
/// down cursor, recording, and config ownership. A weak registry avoids keeping
/// a stopped daemon alive through the process-global hook list.
fn register_state_file_session_end_hook(registry: &std::sync::Arc<cu_core::tool::ToolRegistry>) {
    let registry = std::sync::Arc::downgrade(registry);
    cu_core::session::register_session_end_hook(move |session_id| {
        if let Some(registry) = registry.upgrade() {
            registry.remove_session_state_file(session_id);
        }
    });
}

// ── Paths ─────────────────────────────────────────────────────────────────────

/// Returns the platform default socket/pipe path.
pub fn default_socket_path() -> String {
    pip_preview::home::home()
        .join("cu.sock")
        .to_string_lossy()
        .into_owned()
}

/// Returns the platform default PID file path.
pub fn default_pid_file_path() -> String {
    pip_preview::home::home()
        .join("cu.pid")
        .to_string_lossy()
        .into_owned()
}

fn is_managed_socket_path(path: &str) -> bool {
    path == default_socket_path()
}

fn is_managed_pid_file_path(path: &str) -> bool {
    path == default_pid_file_path()
}

// ── Protocol types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRequest {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    /// MCP-session identity minted once per `brigadier-cu mcp` proxy process and
    /// stamped on every forwarded request. The daemon uses it to OWN and CLEAN
    /// UP session-scoped state (recording, config overrides). Absent (`None`)
    /// means an anonymous/"global" session — a one-shot `brigadier-cu call` or a
    /// legacy proxy that predates this field — which keeps today's behavior.
    /// `skip_serializing_if = Option::is_none` makes the absent case
    /// byte-identical on the wire, so old clients and daemons interoperate.
    /// serde defaults a missing field to `None` on deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Serialize)]
struct AuthenticatedDaemonRequest<'a> {
    auth_token: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_auth_token: Option<&'a str>,
    request: &'a DaemonRequest,
}

#[derive(Debug)]
struct ParsedDaemonRequest {
    request: DaemonRequest,
    host_authenticated: bool,
}

/// The agent token: from the environment, else from `<home>/auth`.
///
/// Brigadier writes `<home>/auth` (mode 0600) and also hands the token to
/// each MCP shim through its environment. The daemon is started through
/// LaunchServices, which drops the environment, so it reads the file.
fn configured_socket_auth_token() -> Option<String> {
    std::env::var(SOCKET_AUTH_TOKEN_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::fs::read_to_string(pip_preview::home::home().join("auth"))
                .ok()
                .map(|token| token.trim().to_owned())
                .filter(|token| !token.is_empty())
        })
}

fn configured_socket_host_auth_token() -> Option<String> {
    std::env::var(SOCKET_HOST_AUTH_TOKEN_ENV)
        .ok()
        .filter(|value| !value.is_empty())
}

fn auth_tokens_match(expected: &str, provided: &str) -> bool {
    let expected = expected.as_bytes();
    let provided = provided.as_bytes();
    let mut difference = expected.len() ^ provided.len();
    for index in 0..expected.len().max(provided.len()) {
        difference |= usize::from(
            expected.get(index).copied().unwrap_or(0) ^ provided.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn socket_peer_requires_authorized_root(
    authorized_root_configured: bool,
    host_authority_configured: bool,
) -> bool {
    authorized_root_configured && !host_authority_configured
}

fn host_request_is_authorized(
    host_authority_configured: bool,
    host_authenticated: bool,
    peer_is_authorized_root: bool,
) -> bool {
    if host_authority_configured {
        host_authenticated
    } else {
        peer_is_authorized_root
    }
}

fn state_authentication_key(args: Option<&serde_json::Value>) -> Option<Vec<u8>> {
    let encoded = args?
        .get("key_base64")
        .and_then(serde_json::Value::as_str)?;
    let key = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()?;
    (key.len() == 32).then_some(key)
}

#[cfg(test)]
fn process_descends_from(
    mut process_id: u32,
    root_process_id: u32,
    parent_process_id: impl Fn(u32) -> Option<u32>,
) -> bool {
    if root_process_id <= 1 {
        return false;
    }
    for _ in 0..128 {
        if process_id == root_process_id {
            return true;
        }
        if process_id <= 1 {
            return false;
        }
        let Some(parent) = parent_process_id(process_id) else {
            return false;
        };
        if parent == process_id {
            return false;
        }
        process_id = parent;
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessIdentity {
    process_id: u32,
    parent_process_id: u32,
    pub(crate) start_seconds: i64,
    pub(crate) start_microseconds: i64,
}

impl ProcessIdentity {
    fn is_same_generation_as(self, other: Self) -> bool {
        self.process_id == other.process_id
            && self.start_seconds == other.start_seconds
            && self.start_microseconds == other.start_microseconds
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_process_identity(process_id: u32) -> Option<ProcessIdentity> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let expected_size = std::mem::size_of::<libc::proc_bsdinfo>();
    let result = unsafe {
        libc::proc_pidinfo(
            i32::try_from(process_id).ok()?,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            i32::try_from(expected_size).ok()?,
        )
    };
    if usize::try_from(result).ok()? != expected_size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != process_id {
        return None;
    }
    Some(ProcessIdentity {
        process_id,
        parent_process_id: info.pbi_ppid,
        start_seconds: i64::try_from(info.pbi_start_tvsec).ok()?,
        start_microseconds: i64::try_from(info.pbi_start_tvusec).ok()?,
    })
}

#[cfg(target_os = "macos")]
fn configured_authorized_root_identity() -> anyhow::Result<Option<ProcessIdentity>> {
    let raw_pid = std::env::var(SOCKET_AUTHORIZED_ROOT_PID_ENV)
        .ok()
        .filter(|value| !value.is_empty());
    let raw_seconds = std::env::var(SOCKET_AUTHORIZED_ROOT_START_SECONDS_ENV)
        .ok()
        .filter(|value| !value.is_empty());
    let raw_microseconds = std::env::var(SOCKET_AUTHORIZED_ROOT_START_MICROSECONDS_ENV)
        .ok()
        .filter(|value| !value.is_empty());
    if raw_pid.is_none() && raw_seconds.is_none() && raw_microseconds.is_none() {
        return Ok(None);
    }
    let process_id = raw_pid
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 1)
        .ok_or_else(|| anyhow::anyhow!("Invalid {SOCKET_AUTHORIZED_ROOT_PID_ENV}"))?;
    let start_seconds = raw_seconds
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| anyhow::anyhow!("Invalid {SOCKET_AUTHORIZED_ROOT_START_SECONDS_ENV}"))?;
    let start_microseconds = raw_microseconds
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| (0..1_000_000).contains(value))
        .ok_or_else(|| {
            anyhow::anyhow!("Invalid {SOCKET_AUTHORIZED_ROOT_START_MICROSECONDS_ENV}")
        })?;
    let configured = ProcessIdentity {
        process_id,
        parent_process_id: 0,
        start_seconds,
        start_microseconds,
    };
    let current = macos_process_identity(process_id)
        .filter(|identity| identity.is_same_generation_as(configured))
        .ok_or_else(|| anyhow::anyhow!("Configured socket root process is no longer live"))?;
    Ok(Some(current))
}

#[cfg(target_os = "macos")]
fn macos_peer_process_id(stream: &tokio::net::UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd;

    let mut pid: libc::pid_t = 0;
    let mut length = libc::socklen_t::try_from(std::mem::size_of_val(&pid)).ok()?;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if result != 0 || pid <= 1 {
        return None;
    }
    u32::try_from(pid).ok()
}

#[cfg(target_os = "macos")]
fn macos_process_is_authorized(
    peer_process_identity: Option<ProcessIdentity>,
    root_process_identity: ProcessIdentity,
) -> bool {
    let Some(mut current) = peer_process_identity else {
        return false;
    };
    for _ in 0..128 {
        if current.process_id == root_process_identity.process_id {
            return current.is_same_generation_as(root_process_identity);
        }
        if current.process_id <= 1 || current.parent_process_id <= 1 {
            return false;
        }
        let Some(parent) = macos_process_identity(current.parent_process_id) else {
            return false;
        };
        if parent.process_id == current.process_id {
            return false;
        }
        current = parent;
    }
    false
}

/// Replaces any caller-supplied state provenance with a kernel-authenticated
/// process generation. A managed proxy may nominate its long-lived agent PID;
/// the daemon accepts it only when that exact live process is an ancestor of
/// the kernel-reported socket peer. This lets authenticated activity outlive a
/// short-lived proxy without allowing a client to nominate a sibling process.
fn attach_state_writer_identity(
    args: &mut serde_json::Value,
    peer_process_identity: Option<ProcessIdentity>,
) {
    let Some(args) = args.as_object_mut() else {
        return;
    };
    let requested_owner_pid = args
        .remove(cu_core::session_state::STATE_OWNER_PID_ARG)
        .and_then(|value| value.as_u64())
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 1);
    args.remove(cu_core::session_state::STATE_WRITER_PID_ARG);
    args.remove(cu_core::session_state::STATE_WRITER_START_SECONDS_ARG);
    args.remove(cu_core::session_state::STATE_WRITER_START_MICROSECONDS_ARG);
    #[cfg(target_os = "macos")]
    let writer_identity = requested_owner_pid
        .and_then(macos_process_identity)
        .filter(|owner| macos_process_is_authorized(peer_process_identity, *owner))
        .or(peer_process_identity);
    if let Some(identity) = writer_identity.filter(|identity| identity.process_id > 1) {
        args.insert(
            cu_core::session_state::STATE_WRITER_PID_ARG.to_owned(),
            serde_json::json!(identity.process_id),
        );
        args.insert(
            cu_core::session_state::STATE_WRITER_START_SECONDS_ARG.to_owned(),
            serde_json::json!(identity.start_seconds),
        );
        args.insert(
            cu_core::session_state::STATE_WRITER_START_MICROSECONDS_ARG.to_owned(),
            serde_json::json!(identity.start_microseconds),
        );
    }
}

/// Serialize one daemon request, adding the authenticated envelope when the
/// embedding host configured a socket credential.
pub fn serialize_request(req: &DaemonRequest) -> anyhow::Result<String> {
    match configured_socket_auth_token() {
        Some(token) => {
            let host_token = configured_socket_host_auth_token();
            Ok(serde_json::to_string(&AuthenticatedDaemonRequest {
                auth_token: &token,
                host_auth_token: host_token.as_deref(),
                request: req,
            })?)
        }
        None => Ok(serde_json::to_string(req)?),
    }
}

fn parse_request(line: &str) -> Result<ParsedDaemonRequest, DaemonResponse> {
    let configured_token = configured_socket_auth_token();
    let configured_host_token = configured_socket_host_auth_token();
    parse_request_with_tokens(
        line,
        configured_token.as_deref(),
        configured_host_token.as_deref(),
    )
}

#[cfg(test)]
fn parse_request_with_token(
    line: &str,
    expected_token: Option<&str>,
) -> Result<DaemonRequest, DaemonResponse> {
    parse_request_with_tokens(line, expected_token, None).map(|parsed| parsed.request)
}

fn parse_request_with_tokens(
    line: &str,
    expected_token: Option<&str>,
    expected_host_token: Option<&str>,
) -> Result<ParsedDaemonRequest, DaemonResponse> {
    let value: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| DaemonResponse::err(format!("JSON parse error: {error}"), 65))?;
    let host_authenticated = expected_host_token.is_some_and(|expected| {
        let provided = value
            .get("host_auth_token")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        auth_tokens_match(expected, provided)
    });
    let request = if let Some(expected) = expected_token {
        let provided = value
            .get("auth_token")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if !auth_tokens_match(expected, provided) {
            return Err(DaemonResponse::err("Unauthorized daemon client", 77));
        }
        value
            .get("request")
            .cloned()
            .ok_or_else(|| DaemonResponse::err("Authenticated request is missing `request`", 65))?
    } else {
        value
    };
    let request = serde_json::from_value(request)
        .map_err(|error| DaemonResponse::err(format!("JSON parse error: {error}"), 65))?;
    Ok(ParsedDaemonRequest {
        request,
        host_authenticated,
    })
}

/// Keep permission prompting under an embedding application's explicit UX.
///
/// The daemon remains the permission-status authority, but an agent cannot
/// bypass the host's onboarding by sending `check_permissions {prompt:true}`.
fn clamp_external_permission_prompt(
    external_permission_flow: bool,
    tool_name: &str,
    args: &mut serde_json::Value,
) {
    if !external_permission_flow || tool_name != "check_permissions" {
        return;
    }
    if let Some(object) = args.as_object_mut() {
        object.insert("prompt".to_owned(), serde_json::Value::Bool(false));
    } else {
        *args = serde_json::json!({ "prompt": false });
    }
}

/// Requests one macOS TCC grant from the standalone helper process.
///
/// This daemon-only method is intentionally separate from the MCP tool
/// registry. Embedding hosts can present their own explicit onboarding UI and
/// ask the correctly attributed helper to raise one native permission request,
/// while `check_permissions { prompt: true }` remains clamped for agents.
fn validate_system_permission_request(permission: Option<&str>) -> DaemonResponse {
    #[cfg(target_os = "macos")]
    {
        match permission {
            Some("accessibility" | "screen_recording") => {}
            Some(name) => {
                return DaemonResponse::err(format!("Unknown system permission: {name}"), 64);
            }
            None => {
                return DaemonResponse::err("System permission request is missing `name`", 65);
            }
        }
        DaemonResponse::ok(serde_json::json!({
            "permission": permission,
            "requested": true,
        }))
    }
}

/// `Err` carries a `capture_timeout` payload: the probe got no answer, so
/// readiness is unknown (`capturable: null`), never "not capturable".
fn screen_capture_verification_response(probe: Result<bool, serde_json::Value>) -> DaemonResponse {
    match probe {
        Ok(capturable) => DaemonResponse::ok(serde_json::json!({
            "capturable": capturable,
        })),
        Err(capture_error) => DaemonResponse::ok(serde_json::json!({
            "capturable": null,
            "capture_error": capture_error,
        })),
    }
}

#[cfg(target_os = "macos")]
fn request_validated_system_permission(permission: &str) {
    match permission {
        "accessibility" => {
            let _ = platform_macos::permissions::request_accessibility();
        }
        "screen_recording" => {
            let _ = platform_macos::permissions::request_screen_recording();
        }
        _ => unreachable!("permission was validated before prompting"),
    }
}

impl DaemonResponse {
    pub fn ok(result: serde_json::Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
            exit_code: None,
        }
    }
    pub fn err(msg: impl Into<String>, code: i32) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(msg.into()),
            exit_code: Some(code),
        }
    }

    #[cfg(test)]
    pub fn tool_error(result: serde_json::Value, msg: impl Into<String>, code: i32) -> Self {
        Self {
            ok: false,
            result: Some(result),
            error: Some(msg.into()),
            exit_code: Some(code),
        }
    }
}

#[cfg(target_os = "macos")]
fn daemon_instance_id() -> &'static str {
    static INSTANCE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(|| {
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}-{started}", std::process::id())
    })
}

#[cfg(target_os = "macos")]
fn daemon_permission_attribution() -> &'static str {
    let inside_bundle = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .map(|path| {
            path.to_string_lossy()
                .contains("/Brigadier Computer Use.app/Contents/MacOS/")
        })
        .unwrap_or(false);
    let launched_by_launchd = unsafe { libc::getppid() } == 1;
    let disclaimed = std::env::var_os(cu_core::RESPONSIBILITY_DISCLAIMED_ENV).is_some();
    if inside_bundle && (launched_by_launchd || disclaimed) {
        "driver-daemon"
    } else {
        "caller"
    }
}

#[cfg(target_os = "macos")]
fn daemon_permission_status(
    profile: DaemonProfile,
    external_permission_ready: bool,
) -> serde_json::Value {
    let status = platform_macos::permissions::current_status();
    crate::lifecycle::record_permissions(status.accessibility, status.screen_recording);
    let panel = platform_macos::permissions::panel::lifecycle();
    let recovery = platform_macos::permissions::gate::recovery_lifecycle();
    serde_json::json!({
        "accessibility": status.accessibility,
        "screen_recording": status.screen_recording,
        "all_granted": status.all_granted(),
        "external_permission_ready": external_permission_ready,
        "profile": profile,
        "panel": {
            "visible": panel.visible,
            "completion": panel.completion,
            "last_outcome": panel.last_outcome.map(
                platform_macos::permissions::panel::outcome_name
            ),
        },
        "recovery": {
            "generation": recovery.generation,
            "state": recovery.state,
        },
        "source": {
            "attribution": daemon_permission_attribution(),
            "pid": std::process::id(),
            "instance": daemon_instance_id(),
        },
    })
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Probe whether a daemon is listening on `socket_path`.
///
/// On Windows this uses `WaitNamedPipeW` with a tiny timeout, which checks
/// whether a pipe instance is available **without consuming one**. The
/// previous design (sending a `list` request) opened a pipe instance,
/// then the immediately-following real `send_request` had to wait for the
/// daemon to spin up its NEXT instance — that race caused real tool calls
/// (especially state-dependent ones like `click` that need the daemon's
/// element_index cache) to fall through to the in-process path with a
/// fresh empty cache. See the conversation around the
/// "Element 3 not in cache" bug for the diagnosis.
///
/// `WaitNamedPipeW(name, 1)`:
///   - Returns TRUE if an instance is currently available.
///   - Returns FALSE if not available within 1 ms.
///   - Does NOT consume the instance — that's reserved for the actual call.
pub fn is_daemon_listening(socket_path: &str) -> bool {
    #[cfg(not(target_os = "windows"))]
    {
        let req = DaemonRequest {
            method: "list".into(),
            name: None,
            args: None,
            session_id: None,
        };
        send_request(socket_path, &req)
            .ok()
            .map(|r| r.ok)
            .unwrap_or(false)
    }
}

/// Read the PID stored in `pid_file_path`, if any.
pub fn read_pid_file(pid_file_path: &str) -> Option<u32> {
    std::fs::read_to_string(pid_file_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// A request that never reached the daemon: nothing it asked for ran, so it
/// can be sent again safely. [`send_request`] returns it inside its error.
#[derive(Debug)]
pub struct NotDelivered(pub String);

impl std::fmt::Display for NotDelivered {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for NotDelivered {}

/// Whether `error` from [`send_request`] means the request never reached the
/// daemon. Any later failure may come after the daemon acted on it.
pub fn never_delivered(error: &anyhow::Error) -> bool {
    error.downcast_ref::<NotDelivered>().is_some()
}

/// Send a request to the daemon and return the response.
/// Uses a 3-second connect timeout (by polling) and a 10-second read timeout.
#[cfg(unix)]
pub fn send_request(socket_path: &str, req: &DaemonRequest) -> anyhow::Result<DaemonResponse> {
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    let mut stream = UnixStream::connect(socket_path)
        .map_err(|e| anyhow::Error::new(NotDelivered(format!("connect to {socket_path}: {e}"))))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    // Per-read timeout acts as a liveness poll, NOT a hard cap on the whole
    // response: an AX-heavy `get_window_state` (slow tree walk) or a multi-MB
    // SOM screenshot can legitimately take longer than one window to produce.
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;

    let mut w = stream.try_clone()?;
    let line = serialize_request(req)? + "\n";
    // EAGAIN-aware write: a daemon momentarily too busy to read our request
    // (backpressure under concurrent slow tools) makes the 5s SO_SNDTIMEO write
    // time out. Treat that like the read loop below does (#1997) — keep retrying
    // until an overall deadline — instead of failing with a fatal "Resource
    // temporarily unavailable (os error 35)" transport error. Mirrors the 120s
    // read budget.
    let write_deadline = Instant::now() + Duration::from_secs(120);
    cu_core::socket_io::write_all_with_retry(&mut w, line.as_bytes(), write_deadline)?;

    // Read the single newline-terminated response line. A blocking UnixStream
    // with SO_RCVTIMEO returns `WouldBlock`/`TimedOut` (EAGAIN, os error 35)
    // when the timeout elapses with no bytes ready — that is NOT a transport
    // failure, just "the daemon is still working". Previously this surfaced as
    // a fatal `daemon transport error … Resource temporarily unavailable`
    // (#1864). Loop and keep waiting (re-arming the per-read poll) until we
    // have a full line, the daemon closes the connection, or an overall
    // deadline generous enough for the slowest AX walk / largest response.
    let overall_deadline = Instant::now() + Duration::from_secs(120);
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 64 * 1024];
    let resp_line = loop {
        if let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            break String::from_utf8_lossy(&buf[..nl]).into_owned();
        }
        match stream.read(&mut chunk) {
            Ok(0) => {
                // EOF. Use whatever we buffered (some daemons close right after
                // a final unterminated line); otherwise the daemon hung up.
                if buf.is_empty() {
                    anyhow::bail!("daemon closed connection without response");
                }
                break String::from_utf8_lossy(&buf).into_owned();
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= overall_deadline {
                    anyhow::bail!(
                        "timed out after 120s waiting for daemon response \
                         (received {} bytes so far)",
                        buf.len()
                    );
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    };
    let resp: DaemonResponse = serde_json::from_str(&resp_line)?;
    Ok(resp)
}

// ── Server ────────────────────────────────────────────────────────────────────

/// Run the daemon server. Binds `socket_path`, writes `pid_file_path`,
/// accepts connections, and serves requests until `{"method":"shutdown"}`.
///
/// This is `async` and must be called from a tokio runtime.
#[cfg(unix)]
#[cfg(test)]
pub async fn run_serve(
    registry: std::sync::Arc<cu_core::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
    profile: DaemonProfile,
) -> anyhow::Result<()> {
    run_serve_with_owner(registry, socket_path, pid_file_path, profile, None)
        .await
        .map(|_| ())
}

/// Wait for the owner watch to report an exit. Pends forever when there is
/// no owner, or when the watch thread ended without seeing an exit.
#[cfg(unix)]
async fn owner_exited(watch: &mut Option<tokio::sync::oneshot::Receiver<()>>) {
    let result = match watch.as_mut() {
        Some(receiver) => receiver.await,
        None => return std::future::pending().await,
    };
    if result.is_ok() {
        return;
    }
    *watch = None;
    std::future::pending().await
}

/// `run_serve` plus `--owner-pid`: when `owner_pid` is set, the daemon
/// shuts down (and removes its socket and pid file) as soon as that process
/// exits. If the owner is already dead, it returns at once without binding.
#[cfg(unix)]
pub async fn run_serve_with_owner(
    registry: std::sync::Arc<cu_core::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
    profile: DaemonProfile,
    owner_pid: Option<i32>,
) -> anyhow::Result<ServeExit> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    // Start the owner watch before binding, so a daemon whose owner is
    // already gone never creates a socket.
    let mut owner_watch = match owner_pid {
        None => None,
        Some(pid) => {
            match crate::owner_watch::watch(pid) {
                crate::owner_watch::OwnerWatch::Watching(receiver) => Some(receiver),
                crate::owner_watch::OwnerWatch::AlreadyExited => {
                    eprintln!("Brigadier Computer Use daemon: owner process {pid} is not running; exiting.");
                    return Ok(ServeExit::OwnerExited);
                }
                crate::owner_watch::OwnerWatch::Unsupported(reason) => {
                    eprintln!("Brigadier Computer Use daemon: {reason}");
                    None
                }
            }
        }
    };
    let mut exit = ServeExit::Shutdown;

    #[cfg(target_os = "macos")]
    let authorized_root_identity = configured_authorized_root_identity()?;
    let host_authority_configured = configured_socket_host_auth_token().is_some();
    let enforce_authorized_root = socket_peer_requires_authorized_root(
        authorized_root_identity.is_some(),
        host_authority_configured,
    );

    let socket = std::path::Path::new(socket_path);
    let daemon_uid = unsafe { libc::geteuid() };
    #[cfg(target_os = "macos")]
    let enforce_peer_code = crate::bundle::is_executable_inside_brigadier_helper_app();

    // The daemon socket carries live desktop-control authority. Require every
    // socket directory to be private to the current user. The default path may
    // repair permissions left by older releases; custom paths fail closed so a
    // typo cannot silently chmod a caller-managed directory.
    //
    // Every ancestor is checked too, and the bind goes through a descriptor
    // of the checked directory, so a rename of any path component between the
    // check and the bind cannot move the socket into another directory.
    let socket_name = socket
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("socket path has no file name: {socket_path}"))?;
    let socket_dir = socket.parent().unwrap_or_else(|| std::path::Path::new(""));
    let socket_directory =
        open_private_runtime_directory(socket_dir, is_managed_socket_path(socket_path))?;

    // Remove a stale socket file (from a crashed previous daemon), then bind.
    let listener = bind_socket_in_directory(&socket_directory, socket_name, socket)?;
    drop(socket_directory);
    listener.set_nonblocking(true)?;
    let listener = UnixListener::from_std(listener)
        .map_err(|e| anyhow::anyhow!("register {socket_path} with the runtime: {e}"))?;

    eprintln!("Brigadier Computer Use daemon listening on {socket_path}");

    // Write PID file.
    if let Some(pid_path) = pid_file_path {
        let pid = std::path::Path::new(pid_path);
        if let Some(dir) = pid.parent() {
            secure_runtime_directory(dir, is_managed_pid_file_path(pid_path))?;
        }
        write_private_pid_file(pid)?;
    }

    // Shutdown channel.
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(shutdown_tx)));

    // Idle-TTL recording backstop (#1764), now demoted to SECONDARY cover. The
    // PRIMARY teardown is the per-session reaper: the proxy holds a persistent
    // control connection (session_begin) whose EOF — on graceful exit AND on
    // kill -9 — fires session_end, stopping that session's recording reliably.
    // This TTL still uniquely covers (a) a non-proxy raw client that starts a
    // recording and dies without ever sending session_begin and (b) a
    // daemon-internal wedge. We track the last `call` activity timestamp; a
    // background task auto-stops the recording after `RECORDING_IDLE_TTL_SECS`
    // of zero tool activity. Keyed on call activity (NOT connection liveness),
    // so an actively-used session is never reaped.
    let last_activity = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_unix_secs()));
    spawn_recording_idle_backstop(registry.clone(), last_activity.clone());
    spawn_session_idle_sweep();
    register_recording_session_end_hook(registry.recording.clone());
    register_state_file_session_end_hook(&registry);
    let activity = crate::activity_daemon::open(profile, socket_path);
    let mut authorized_root_health = tokio::time::interval(std::time::Duration::from_secs(1));

    // Setup Brigadier already finished holds for this daemon too (one started
    // after a restart or an update).
    let external_permission_ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        crate::bundle::is_env_truthy(crate::lifecycle::EXTERNAL_SETUP_ENV)
            && crate::lifecycle::capture_confirmed(&crate::lifecycle::home()),
    ));

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                if let Err(refusal) = check_peer_uid(&stream, daemon_uid) {
                    eprintln!("brigadier-cu: refused daemon socket peer: {refusal}");
                    continue;
                }
                #[cfg(target_os = "macos")]
                let peer_process_id = macos_peer_process_id(&stream);
                // A daemon running from the signed helper app only talks to
                // processes signed the same way: its own shim and CLI. Any
                // other same-user process is refused even if it found the token.
                #[cfg(target_os = "macos")]
                if enforce_peer_code {
                    let verdict = peer_process_id
                        .ok_or_else(|| "could not resolve the peer pid".to_owned())
                        .and_then(|pid| {
                            validate_peer_matches_daemon_code(pid as libc::pid_t)
                        });
                    if let Err(refusal) = verdict {
                        eprintln!("brigadier-cu: refused daemon socket peer: {refusal}");
                        continue;
                    }
                }
                #[cfg(target_os = "macos")]
                let peer_process_identity =
                    peer_process_id.and_then(macos_process_identity);
                #[cfg(target_os = "macos")]
                if enforce_authorized_root {
                    if let Some(root_process_identity) = authorized_root_identity {
                        if !macos_process_is_authorized(
                            peer_process_identity,
                            root_process_identity,
                        ) {
                            eprintln!("Rejected unauthorized daemon peer");
                            continue;
                        }
                    }
                }
                #[cfg(target_os = "macos")]
                let peer_is_authorized_root = match (
                    peer_process_identity,
                    authorized_root_identity,
                ) {
                    (Some(peer), Some(root)) => peer.is_same_generation_as(root),
                    (_, None) => true,
                    _ => false,
                };
                let reg = registry.clone();
                let shutdown_tx2 = shutdown_tx.clone();
                let last_activity = last_activity.clone();
                let activity = activity.clone();
                let activity_peer_pid = stream
                    .peer_cred()
                    .ok()
                    .and_then(|credentials| credentials.pid())
                    .and_then(|pid| u32::try_from(pid).ok());
                let external_permission_ready = external_permission_ready.clone();

                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut lines = BufReader::new(reader).lines();

                    // Set ONLY by a `session_begin` on this connection — i.e.
                    // the proxy's persistent control connection. Per-call
                    // connections (call/list/describe) leave this None, so
                    // their immediate EOF triggers NO teardown. When a control
                    // connection EOFs (graceful proxy exit OR kill -9, both
                    // kernel-guaranteed), the post-loop block reaps the session.
                    let mut control_session_id: Option<String> = None;

                    while let Ok(Some(line)) = lines.next_line().await {
                        #[cfg(target_os = "macos")]
                        if let Some(root_process_identity) = authorized_root_identity {
                            let root_is_still_live =
                                macos_process_identity(root_process_identity.process_id)
                                    .is_some_and(|identity| {
                                        identity.is_same_generation_as(root_process_identity)
                                    });
                            if !root_is_still_live {
                                return;
                            }
                        }
                        let parsed = match parse_request(&line) {
                            Ok(request) => request,
                            Err(resp) => {
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                continue;
                            }
                        };
                        let host_request_authorized = host_request_is_authorized(
                            host_authority_configured,
                            parsed.host_authenticated,
                            peer_is_authorized_root,
                        );
                        let req = parsed.request;

                        match req.method.as_str() {
                            "shutdown" => {
                                if !host_request_authorized {
                                    let resp = DaemonResponse::err(
                                        "Unauthorized host-only daemon request".to_owned(),
                                        77,
                                    );
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                let resp = DaemonResponse::ok(serde_json::json!({"shutdown": true}));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                let mut guard = shutdown_tx2.lock().await;
                                if let Some(tx) = guard.take() {
                                    let _ = tx.send(());
                                }
                                return;
                            }
                            "permissions_status" => {
                                #[cfg(target_os = "macos")]
                                let resp = DaemonResponse::ok(daemon_permission_status(
                                    profile,
                                    external_permission_ready.load(
                                        std::sync::atomic::Ordering::Acquire
                                    ),
                                ));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "set_external_permission_ready" => {
                                let response = if !host_request_authorized {
                                    DaemonResponse::err(
                                        "Unauthorized host-only daemon request".to_owned(),
                                        77,
                                    )
                                } else if let Some(ready) = req
                                    .args
                                    .as_ref()
                                    .and_then(|args| args.get("ready"))
                                    .and_then(serde_json::Value::as_bool)
                                {
                                    external_permission_ready.store(
                                        ready,
                                        std::sync::atomic::Ordering::Release,
                                    );
                                    DaemonResponse::ok(serde_json::json!({
                                        "external_permission_ready": ready,
                                    }))
                                } else {
                                    DaemonResponse::err(
                                        "External permission readiness requires a boolean `ready`"
                                            .to_owned(),
                                        64,
                                    )
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&response).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "permissions_present" => {
                                #[cfg(target_os = "macos")]
                                {
                                    let prepared =
                                        platform_macos::permissions::gate::prepare_interactive_recovery();
                                    match prepared {
                                        Ok(request) => {
                                            let resp = DaemonResponse::ok(serde_json::json!({
                                                "presentation": request.state(),
                                                "generation": request.generation(),
                                                "profile": profile,
                                            }));
                                            let _ = writer.write_all(
                                                (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                            ).await;
                                            let _ = writer.flush().await;
                                            platform_macos::permissions::gate::dispatch_prepared_interactive_recovery(
                                                request,
                                            );
                                        }
                                        Err(error) => {
                                            let resp = DaemonResponse::err(error.to_string(), 1);
                                            let _ = writer.write_all(
                                                (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                            ).await;
                                        }
                                    }
                                }
                            }
                            "permissions_refresh" => {
                                #[cfg(target_os = "macos")]
                                {
                                    let available =
                                        platform_macos::permissions::gate::ensure_process_refresh_available();
                                    match available {
                                        Ok(()) => {
                                            let resp = DaemonResponse::ok(serde_json::json!({
                                                "refresh": "scheduled",
                                                "profile": profile,
                                                "pid": std::process::id(),
                                            }));
                                            let _ = writer.write_all(
                                                (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                            ).await;
                                            let _ = writer.flush().await;
                                            platform_macos::permissions::gate::dispatch_process_refresh();
                                        }
                                        Err(error) => {
                                            let resp = DaemonResponse::err(error.to_string(), 1);
                                            let _ = writer.write_all(
                                                (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                            ).await;
                                        }
                                    }
                                }
                            }
                            "request_system_permission" => {
                                if !host_request_authorized {
                                    let resp = DaemonResponse::err(
                                        "Unauthorized host-only daemon request".to_owned(),
                                        77,
                                    );
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                let resp = validate_system_permission_request(req.name.as_deref());
                                let response_sent = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await.is_ok();
                                #[cfg(target_os = "macos")]
                                if resp.ok && response_sent {
                                    request_validated_system_permission(
                                        req.name.as_deref().expect("validated permission has a name")
                                    );
                                }
                            }
                            "application_windows" => {
                                #[cfg(target_os = "macos")]
                                let resp = if !host_request_authorized
                                    || profile != DaemonProfile::Native
                                {
                                    DaemonResponse::err(
                                        "Unauthorized host-only application-surface request",
                                        77,
                                    )
                                } else {
                                    match tokio::task::spawn_blocking(
                                        platform_macos::application_surface::list_windows,
                                    ).await {
                                        Ok(Ok(windows)) => DaemonResponse::ok(
                                            serde_json::json!({"windows": windows}),
                                        ),
                                        Ok(Err(error)) => {
                                            DaemonResponse::err(format!("{error:#}"), 1)
                                        }
                                        Err(error) => DaemonResponse::err(
                                            format!("Application-window task failed: {error}"),
                                            1,
                                        ),
                                    }
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "application_surface_start" => {
                                #[cfg(target_os = "macos")]
                                let resp = if !host_request_authorized
                                    || profile != DaemonProfile::Native
                                {
                                    DaemonResponse::err(
                                        "Unauthorized host-only application-surface request",
                                        77,
                                    )
                                } else {
                                    let request = req.args
                                        .ok_or("Missing application-surface arguments")
                                        .and_then(|args| {
                                            serde_json::from_value::<
                                                platform_macos::application_surface::
                                                    ApplicationSurfaceStartRequest,
                                            >(args)
                                            .map_err(|_| {
                                                "Invalid application-surface arguments"
                                            })
                                        });
                                    match request {
                                        Ok(request) => match tokio::task::spawn_blocking(
                                            move || {
                                                platform_macos::application_surface::start(request)
                                            },
                                        ).await {
                                            Ok(Ok(result)) => DaemonResponse::ok(
                                                serde_json::to_value(result).expect(
                                                    "application start result is serializable"
                                                ),
                                            ),
                                            Ok(Err(error)) => {
                                                DaemonResponse::err(format!("{error:#}"), 1)
                                            }
                                            Err(error) => DaemonResponse::err(
                                                format!(
                                                    "Application-capture task failed: {error}"
                                                ),
                                                1,
                                            ),
                                        },
                                        Err(error) => DaemonResponse::err(error, 64),
                                    }
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "application_surface_stop" => {
                                #[cfg(target_os = "macos")]
                                let resp = if !host_request_authorized
                                    || profile != DaemonProfile::Native
                                {
                                    DaemonResponse::err(
                                        "Unauthorized host-only application-surface request",
                                        77,
                                    )
                                } else {
                                    let session = req.args.as_ref()
                                        .and_then(|args| args.get("session"))
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::trim)
                                        .filter(|value| {
                                            !value.is_empty() && value.len() <= 128
                                        })
                                        .map(str::to_owned);
                                    match session {
                                        Some(session) => {
                                            let stopped = tokio::task::spawn_blocking(
                                                move || {
                                                    platform_macos::application_surface::stop(
                                                        &session,
                                                    )
                                                },
                                            ).await.unwrap_or(false);
                                            DaemonResponse::ok(
                                                serde_json::json!({"stopped": stopped}),
                                            )
                                        }
                                        None => DaemonResponse::err(
                                            "application_surface_stop requires a session",
                                            64,
                                        ),
                                    }
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "application_surface_event" => {
                                #[cfg(target_os = "macos")]
                                let resp = if !host_request_authorized
                                    || profile != DaemonProfile::Native
                                {
                                    DaemonResponse::err(
                                        "Unauthorized host-only application-surface request",
                                        77,
                                    )
                                } else {
                                    let event = req.args
                                        .ok_or("Missing application-surface event")
                                        .and_then(|args| {
                                            serde_json::from_value::<
                                                platform_macos::application_surface::
                                                    ApplicationSurfaceEvent,
                                            >(args)
                                            .map_err(|_| "Invalid application-surface event")
                                        });
                                    match event {
                                        Ok(event) => match tokio::task::spawn_blocking(
                                            move || {
                                                platform_macos::application_surface::send_event(
                                                    event,
                                                )
                                            },
                                        ).await {
                                            Ok(Ok(())) => DaemonResponse::ok(
                                                serde_json::json!({"sent": true}),
                                            ),
                                            Ok(Err(error)) => {
                                                DaemonResponse::err(format!("{error:#}"), 1)
                                            }
                                            Err(error) => DaemonResponse::err(
                                                format!(
                                                    "Application-input task failed: {error}"
                                                ),
                                                1,
                                            ),
                                        },
                                        Err(error) => DaemonResponse::err(error, 64),
                                    }
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "verify_screen_capture" => {
                                if !host_request_authorized {
                                    let resp = DaemonResponse::err(
                                        "Unauthorized host-only daemon request".to_owned(),
                                        77,
                                    );
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                #[cfg(target_os = "macos")]
                                let resp = {
                                    let probe = tokio::task::spawn_blocking(
                                        platform_macos::tools::verify_screen_capture_ready,
                                    )
                                    .await
                                    .unwrap_or(Ok(false))
                                    .map_err(|timeout| timeout.to_json());
                                    screen_capture_verification_response(probe)
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "configure_state_authentication" => {
                                let response = if !host_request_authorized
                                    || (!host_authority_configured
                                        && authorized_root_identity.is_none())
                                {
                                    DaemonResponse::err(
                                        "Unauthorized host-only daemon request".to_owned(),
                                        77,
                                    )
                                } else if let Some(key) =
                                    state_authentication_key(req.args.as_ref())
                                {
                                    if reg.set_state_authentication_key(key) {
                                        DaemonResponse::ok(serde_json::json!({
                                            "state_authentication": true
                                        }))
                                    } else {
                                        DaemonResponse::err(
                                            "State authentication is unavailable".to_owned(),
                                            1,
                                        )
                                    }
                                } else {
                                    DaemonResponse::err(
                                        "Invalid state authentication key".to_owned(),
                                        64,
                                    )
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&response).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "set_cursor_enabled" => {
                                #[cfg(target_os = "macos")]
                                let response = if !host_request_authorized {
                                    DaemonResponse::err(
                                        "Unauthorized host-only daemon request".to_owned(),
                                        77,
                                    )
                                } else {
                                    let session = req
                                        .args
                                        .as_ref()
                                        .and_then(|args| args.get("session"))
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::trim)
                                        .filter(|value| {
                                            !value.is_empty() && value.len() <= 1_024
                                        });
                                    let enabled = req
                                        .args
                                        .as_ref()
                                        .and_then(|args| args.get("enabled"))
                                        .and_then(serde_json::Value::as_bool);
                                    let generation = req
                                        .args
                                        .as_ref()
                                        .and_then(|args| args.get("generation"))
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::trim)
                                        .filter(|value| {
                                            !value.is_empty() && value.len() <= 1_024
                                        })
                                        .map(str::to_owned);
                                    match (session, enabled) {
                                        (Some(session), Some(enabled)) => {
                                            platform_macos::cursor::overlay::
                                                set_enabled_for_host_session_generation(
                                                    session.to_owned(),
                                                    enabled,
                                                    generation,
                                                );
                                            DaemonResponse::ok(serde_json::json!({
                                                "session": session,
                                                "cursor_enabled": enabled,
                                            }))
                                        }
                                        _ => DaemonResponse::err(
                                            "set_cursor_enabled requires a non-empty `session` and boolean `enabled`",
                                            64,
                                        ),
                                    }
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&response).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "reassert_cursor" => {
                                #[cfg(target_os = "macos")]
                                let response = if !host_request_authorized {
                                    DaemonResponse::err(
                                        "Unauthorized host-only daemon request".to_owned(),
                                        77,
                                    )
                                } else {
                                    let session = req
                                        .args
                                        .as_ref()
                                        .and_then(|args| args.get("session"))
                                        .and_then(serde_json::Value::as_str)
                                        .map(str::trim)
                                        .filter(|value| {
                                            !value.is_empty() && value.len() <= 1_024
                                        });
                                    let window_id = req
                                        .args
                                        .as_ref()
                                        .and_then(|args| args.get("window_id"))
                                        .and_then(serde_json::Value::as_u64)
                                        .filter(|value| {
                                            *value > 0 && *value <= u64::from(u32::MAX)
                                        });
                                    match (session, window_id) {
                                        (Some(session), Some(window_id)) => {
                                            let reasserted =
                                                platform_macos::cursor::overlay::reassert_for_host_session(
                                                    session.to_owned(),
                                                    window_id,
                                                );
                                            DaemonResponse::ok(serde_json::json!({
                                                "session": session,
                                                "window_id": window_id,
                                                "reasserted": reasserted,
                                            }))
                                        }
                                        _ => DaemonResponse::err(
                                            "reassert_cursor requires a non-empty `session` and positive `window_id`",
                                            64,
                                        ),
                                    }
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&response).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "agent_sessions" => {
                                #[cfg(target_os = "macos")]
                                let running_pid = |bundle_id: &str| {
                                    platform_macos::apps::list_running_apps()
                                        .into_iter()
                                        .find(|app| app.bundle_id.as_deref() == Some(bundle_id))
                                        .map(|app| i64::from(app.pid))
                                };
                                #[cfg(not(target_os = "macos"))]
                                let running_pid = |_: &str| None;
                                let resp = DaemonResponse::ok(crate::agent_apps::list(&running_pid));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "stop_app" => {
                                let field = |key: &str| {
                                    req.args
                                        .as_ref()
                                        .and_then(|args| args.get(key))
                                        .and_then(serde_json::Value::as_str)
                                        .filter(|value| !value.is_empty())
                                        .map(str::to_owned)
                                };
                                let resp = match (host_request_authorized, field("session"), field("app")) {
                                    (false, _, _) => DaemonResponse::err(
                                        "Unauthorized host-only daemon request".to_owned(),
                                        77,
                                    ),
                                    (true, Some(session), Some(app)) => {
                                        let stop = crate::agent_apps::stop(&session, field("bundle_id"), app);
                                        DaemonResponse::ok(serde_json::json!({
                                            "stopped": true,
                                            "shims": stop.shims,
                                        }))
                                    }
                                    _ => DaemonResponse::err(
                                        "stop_app needs `session` and `app`".to_owned(),
                                        64,
                                    ),
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "health" => {
                                // A daemon whose main loop stopped running is
                                // hung even though this socket thread answers.
                                let silence = crate::lifecycle::main_loop_silence();
                                let resp = if silence >= crate::lifecycle::MAIN_LOOP_STALL {
                                    DaemonResponse::err(
                                        format!(
                                            "the main loop has been silent for {}s",
                                            silence.as_secs()
                                        ),
                                        1,
                                    )
                                } else {
                                    DaemonResponse::ok(serde_json::json!({
                                        "build_id": crate::lifecycle::own_build_id(),
                                        "pid": std::process::id(),
                                        "main_loop_silence_ms": silence.as_millis() as u64,
                                    }))
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "list" => {
                                // Include full ToolDef (input_schema + annotation
                                // hints + capabilities) so MCP proxy callers can
                                // build a complete `tools/list` response from
                                // one daemon round-trip. Older clients that only
                                // read name/description still work — the extra
                                // fields are ignored.
                                //
                                // `capabilities` is sourced from the centralised
                                // `cu_core::tool::default_capabilities_for`
                                // name → tokens map so the daemon and in-process
                                // paths emit identical capability arrays.
                                let tools: Vec<serde_json::Value> = reg.iter_defs()
                                    .map(|(name, def)| {
                                        let caps = cu_core::tool::default_capabilities_for(name);
                                        serde_json::json!({
                                            "name": name,
                                            "description": def.description,
                                            "input_schema": def.input_schema,
                                            "read_only": def.read_only,
                                            "destructive": def.destructive,
                                            "idempotent": def.idempotent,
                                            "open_world": def.open_world,
                                            "capabilities": caps,
                                        })
                                    })
                                    .collect();
                                // Mirror `tools_list` envelope: include
                                // capability_version + schema_version so MCP
                                // proxy callers can pass them through verbatim
                                // (one daemon round-trip, complete response).
                                let resp = DaemonResponse::ok(serde_json::json!({
                                    "tools": tools,
                                    "capability_version": cu_core::tool::CAPABILITY_VERSION,
                                    "schema_version": "1",
                                    "profile": profile,
                                }));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "describe" => {
                                let name = req.name.as_deref().unwrap_or("");
                                match reg.get_def(name) {
                                    Some(def) => {
                                        let resp = DaemonResponse::ok(serde_json::json!({
                                            "name": def.name,
                                            "description": def.description,
                                            "input_schema": def.input_schema
                                        }));
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                    }
                                    None => {
                                        let resp = DaemonResponse::err(
                                            format!("Unknown tool: {name}"), 64
                                        );
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                    }
                                }
                            }
                            method if crate::activity_daemon::is_activity_method(method) => {
                                let Some(activity) = activity.as_ref() else {
                                    let resp = DaemonResponse::err("activity store is not available".to_owned(), 69);
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                };
                                let caller = crate::activity_daemon::caller(
                                    activity_peer_pid,
                                    host_request_authorized,
                                );
                                let args = req.args.clone().unwrap_or(serde_json::Value::Null);
                                if method == "activity_subscribe" {
                                    // Stream until the client closes: an initial
                                    // sessions snapshot, then pushed updates.
                                    let sessions = args.get("sessions").and_then(serde_json::Value::as_bool).unwrap_or(true);
                                    let events_for: Vec<String> = args
                                        .get("events_for")
                                        .and_then(serde_json::Value::as_array)
                                        .map(|ids| ids.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                                        .unwrap_or_default();
                                    let mut updates = activity.subscribe();
                                    let snapshot = cu_core::activity::host::ActivityUpdate::Sessions {
                                        sessions: activity.sessions(None, None, 500),
                                    };
                                    if let Some(line) = crate::activity_daemon::stream_line(&snapshot, true, &events_for) {
                                        if writer.write_all(line.as_bytes()).await.is_err() {
                                            return;
                                        }
                                    }
                                    loop {
                                        tokio::select! {
                                            update = updates.recv() => {
                                                let update = match update {
                                                    Ok(update) => update,
                                                    // A slow reader missed updates: resend the full list.
                                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                                        cu_core::activity::host::ActivityUpdate::Sessions {
                                                            sessions: activity.sessions(None, None, 500),
                                                        }
                                                    }
                                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                                                };
                                                if let Some(line) = crate::activity_daemon::stream_line(&update, sessions, &events_for) {
                                                    if writer.write_all(line.as_bytes()).await.is_err() {
                                                        return;
                                                    }
                                                }
                                            }
                                            next = lines.next_line() => {
                                                if !matches!(next, Ok(Some(_))) {
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                }
                                let resp = crate::activity_daemon::handle(activity, method, &args, &caller);
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "call" => {
                                // Turned off in Brigadier: holds even for agents
                                // that were already running.
                                if crate::lifecycle::is_off(&crate::lifecycle::home()) {
                                    let resp = DaemonResponse::ok(serde_json::json!({
                                        "content": [{"type": "text", "text": crate::lifecycle::OFF_MESSAGE}],
                                        "isError": true,
                                    }));
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                let raw_name = req.name.as_deref().unwrap_or("").to_owned();
                                // Deprecated alias: `type_text_chars` → `type_text`.
                                // Mirrors Swift's `ToolRegistry.call` aliasing.
                                let tool_name = if raw_name == "type_text_chars" {
                                    eprintln!("[brigadier-cu] deprecated tool name 'type_text_chars' — use 'type_text' instead.");
                                    "type_text".to_owned()
                                } else { raw_name.clone() };
                                let mut args = req.args.unwrap_or(serde_json::Value::Object(
                                    serde_json::Map::new()
                                ));
                                attach_state_writer_identity(
                                    &mut args,
                                    peer_process_identity,
                                );
                                // Only authenticated calls keep session-owned
                                // recording state alive.
                                last_activity.store(
                                    now_unix_secs(),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                clamp_external_permission_prompt(
                                    crate::bundle::is_env_truthy(
                                        "BRIGADIER_CU_EXTERNAL_PERMISSION_FLOW"
                                    ),
                                    &tool_name,
                                    &mut args,
                                );
                                // Apply the caller-declared session identity
                                // (explicit `session` → `_session_id`; minted id
                                // as the recording/config fallback only). See
                                // `apply_session_identity` for the full rationale
                                // and the cursor's explicit-required contract.
                                let effective_session =
                                    apply_session_identity(&mut args, &req.session_id);
                                // Resurrection guard: a call whose effective
                                // session has already ended (end_session / idle
                                // TTL / control-connection EOF) must NOT run — it
                                // would re-create session-owned state (cursor,
                                // config override, recording) the reaper already
                                // passed. Reject it LOUDLY (isError) so a stray
                                // late action is visibly a failure, not a phantom
                                // success a caller silently trusts. The session-
                                // lifecycle tools are EXEMPT: `start_session`
                                // revives an ended id (explicit, intentional reuse)
                                // and `end_session` stays idempotent. Live and
                                // anonymous calls pass through unchanged.
                                if let Some(sid) = &effective_session {
                                    if !is_session_lifecycle_tool(&tool_name)
                                        && cu_core::session::is_session_ended(sid)
                                    {
                                        let resp = DaemonResponse::err(
                                            format!(
                                                "session '{sid}' has ended; tool call '{tool_name}' was \
                                                 rejected. Call start_session with this id to revive it \
                                                 before issuing further actions, or use a new session id."
                                            ),
                                            1,
                                        );
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                        continue;
                                    }
                                }
                                if reg.get_def(&tool_name).is_none() {
                                    let resp = DaemonResponse::err(
                                        format!("Unknown tool: {tool_name}"), 64
                                    );
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                // Note the app a Brigadier agent acts on, and refuse
                                // one the user stopped it from using.
                                #[cfg(target_os = "macos")]
                                if let Some(host_session) = args
                                    .get(cu_core::HOST_SESSION_ARG)
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_owned)
                                {
                                    let app = platform_macos::target_guard::target_app(&tool_name, &args)
                                        .map(|target| crate::agent_apps::App {
                                            name: target.name,
                                            bundle_id: target.bundle_id,
                                            pid: target.pid,
                                        });
                                    if tool_name == "end_session" {
                                        crate::agent_apps::session_ended(&host_session);
                                    } else if let Some(message) =
                                        crate::agent_apps::note_call(&host_session, activity_peer_pid, app)
                                    {
                                        let resp = DaemonResponse::ok(serde_json::json!({
                                            "content": [{"type": "text", "text": message}],
                                            "isError": true,
                                            "structuredContent": {"error": "stopped_by_user", "message": message},
                                        }));
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                        continue;
                                    }
                                }
                                // Activity log (computer-use.md section 4): the
                                // gate refuses calls the user stopped or paused
                                // and records every call that runs.
                                let activity_label = crate::activity_daemon::label(
                                    effective_session.as_deref(),
                                    req.session_id.as_deref(),
                                );
                                let activity_start = match activity.as_ref() {
                                    Some(host) => {
                                        let caller = crate::activity_daemon::caller(activity_peer_pid, false);
                                        match crate::activity_daemon::begin(host, &caller, &activity_label, &tool_name, &args) {
                                            cu_core::activity::host::Gate::Run(start) => Some(start),
                                            cu_core::activity::host::Gate::Refuse { code, message } => {
                                                let resp = crate::activity_daemon::refusal(&code, &message);
                                                let _ = writer.write_all(
                                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                                ).await;
                                                continue;
                                            }
                                            cu_core::activity::host::Gate::Replay(_)
                                            | cu_core::activity::host::Gate::RunUntracked => None,
                                        }
                                    }
                                    None => None,
                                };
                                let activity_args = activity_start.as_ref().map(|_| args.clone());
                                let result = reg.invoke(&tool_name, args).await;
                                let is_err = result.is_error.unwrap_or(false);
                                let content: Vec<serde_json::Value> = result.content.iter().map(|c| {
                                    match c {
                                        cu_core::protocol::Content::Text { text, .. } =>
                                            serde_json::json!({"type":"text","text":text}),
                                        cu_core::protocol::Content::Image { data, mime_type, .. } =>
                                            serde_json::json!({"type":"image","data":data,"mimeType":mime_type}),
                                    }
                                }).collect();
                                let mut result_obj = serde_json::json!({
                                    "content": content,
                                    "isError": is_err
                                });
                                if let Some(sc) = result.structured_content {
                                    result_obj["structuredContent"] = sc;
                                }
                                if let (Some(host), Some(start), Some(call_args)) =
                                    (activity.clone(), activity_start, activity_args)
                                {
                                    crate::activity_daemon::finish(host, start, &call_args, &result_obj).await;
                                }
                                // Preserve tool-level `isError` and structured
                                // content inside a successful daemon transport.
                                let resp = DaemonResponse::ok(result_obj);
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "session_begin" => {
                                // The proxy's persistent control connection.
                                // Record the session_id so this connection's EOF
                                // (graceful proxy exit OR kill -9) reaps the
                                // session in the post-loop block below. This is
                                // the ONLY place a connection is marked control;
                                // per-call connections never send this. ACK ok.
                                if let Some(sid) = req.session_id.as_deref() {
                                    // A proxy reconnect after daemon re-exec uses the same
                                    // session id. If the old control connection died while
                                    // this daemon stayed alive, its EOF reaper tombstoned the
                                    // session first; the new control declaration revives it
                                    // before accepting more calls.
                                    cu_core::session::revive_session(sid);
                                    #[cfg(target_os = "macos")]
                                    if let Some(host_session) =
                                        cu_core::host_session_id_from_proxy(sid)
                                    {
                                        platform_macos::cursor::overlay::begin_reusable_cursor_generation(
                                            host_session.to_owned(),
                                            sid.to_owned(),
                                        );
                                    }
                                    if control_session_id.as_deref() != Some(sid) {
                                        if let Some(previous) = control_session_id.as_deref() {
                                            cu_core::session::release_session(previous);
                                        }
                                        cu_core::session::hold_session(sid);
                                    }
                                    control_session_id = Some(sid.to_owned());
                                    if let Some(host) = crate::agent_apps::host_session_of(sid) {
                                        crate::agent_apps::shim_connected(host, activity_peer_pid);
                                    }
                                }
                                let resp = DaemonResponse::ok(
                                    serde_json::json!({
                                        "session_begin": true,
                                        "profile": profile,
                                    })
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "session_end" => {
                                // Legacy back-compat: a new-daemon/old-proxy
                                // pairing still sends this explicitly on graceful
                                // stdin EOF. It arrives on a per-call connection
                                // (control_session_id stays None) so it does NOT
                                // double-fire against the EOF reaper, and
                                // fire_session_end is idempotent regardless. New
                                // proxies never send it — the control-connection
                                // EOF is the single teardown path. Always ACK ok.
                                if let Some(sid) = req.session_id.as_deref() {
                                    // stop_owner can SYNCHRONOUSLY finalize the
                                    // recording's mp4 — run it off the reactor on
                                    // a blocking thread (see the EOF reaper).
                                    // fire_session_end stays inline (non-blocking).
                                    // Mark the session ended FIRST so an in-flight
                                    // start_recording sees ended=true and bails — the
                                    // mark-before-reap invariant the cursor/config hooks
                                    // already satisfy (their reap runs inside
                                    // fire_session_end, after the mark). stop_owner does
                                    // not consult is_session_ended, so reaping after the
                                    // mark is safe.
                                    cu_core::session::fire_session_end(sid);
                                    let reg2 = reg.clone();
                                    let sid_for_stop = sid.to_owned();
                                    let _ = tokio::task::spawn_blocking(move || {
                                        reg2.recording.stop_owner(Some(&sid_for_stop))
                                    })
                                    .await;
                                }
                                let resp = DaemonResponse::ok(
                                    serde_json::json!({"session_end": true})
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            other => {
                                let resp = DaemonResponse::err(
                                    format!("Unknown method: {other}"), 65
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                        }
                    }

                    // Reader EOF (`while let` exited): the kernel closes the
                    // socket on graceful proxy exit AND on kill -9. If this was
                    // the proxy's persistent control connection, reap the
                    // session now — the reliable ungraceful-death teardown that
                    // the old graceful-only proxy-exit hook could not provide.
                    // Per-call connections leave control_session_id None, so
                    // their (immediate) EOF is a no-op here. fire_session_end is
                    // idempotent, so racing a legacy explicit session_end is
                    // benign.
                    if let Some(sid) = control_session_id {
                        crate::agent_apps::shim_gone(activity_peer_pid);
                        cu_core::session::release_session(&sid);
                        {
                            // stop_owner can SYNCHRONOUSLY finalize the recording's
                            // mp4, on macOS it hits SCStream::stop_capture(), which
                            // blocks on disk I/O (video_sckit.rs). Run it on a
                            // blocking thread so it does not stall a runtime worker.
                            // fire_session_end stays inline: its hooks (overlay
                            // Remove, config-override clear) are non-blocking.
                            // Mark the session ended FIRST so an in-flight start_recording
                            // sees ended=true and bails (mark-before-reap; the cursor/config
                            // hooks already reap inside fire_session_end after the mark).
                            // stop_owner ignores is_session_ended, so reaping after is safe.
                            cu_core::session::fire_session_end(&sid);
                            let reg2 = reg.clone();
                            let sid_for_stop = sid.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                reg2.recording.stop_owner(Some(&sid_for_stop))
                            })
                            .await;
                        }
                    }
                });
            }
            _ = &mut shutdown_rx => {
                eprintln!("Brigadier Computer Use daemon shutting down.");
                break;
            }
            _ = owner_exited(&mut owner_watch) => {
                eprintln!(
                    "Brigadier Computer Use daemon: owner process {} exited; removing socket and shutting down.",
                    owner_pid.unwrap_or_default()
                );
                exit = ServeExit::OwnerExited;
                break;
            }
            _ = authorized_root_health.tick(), if authorized_root_identity.is_some() => {
                #[cfg(target_os = "macos")]
                if let Some(root_process_identity) = authorized_root_identity {
                    let root_is_still_live =
                        macos_process_identity(root_process_identity.process_id)
                            .is_some_and(|identity| {
                                identity.is_same_generation_as(root_process_identity)
                            });
                    if !root_is_still_live {
                        eprintln!("Authorized root process exited; shutting down daemon.");
                        break;
                    }
                }
            }
        }
    }

    // Clean up.
    #[cfg(target_os = "macos")]
    platform_macos::application_surface::stop_all();
    let _ = std::fs::remove_file(socket_path);
    if let Some(pid_path) = pid_file_path {
        let _ = std::fs::remove_file(pid_path);
    }
    registry.remove_state_file();

    Ok(exit)
}

// ── CLI helpers ───────────────────────────────────────────────────────────────

/// `brigadier-cu serve` implementation.
///
/// With `owner_pid`, the process exits 0 once the owner exits (after the
/// daemon removed its socket), so a blocked main thread cannot keep it alive.
pub fn run_serve_cmd(
    registry: std::sync::Arc<cu_core::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
    profile: DaemonProfile,
    owner_pid: Option<i32>,
) {
    let socket_path = socket_path.to_owned();
    let pid_file_path = pid_file_path.map(str::to_owned);

    // Fail fast if another daemon is already running.
    if is_daemon_listening(&socket_path) {
        let pid_hint = pid_file_path
            .as_deref()
            .and_then(read_pid_file)
            .map(|pid| format!(" (pid {pid})"))
            .unwrap_or_default();
        eprintln!(
            "Brigadier Computer Use daemon is already running on {socket_path}{pid_hint}. \
             Run `brigadier-cu stop` first."
        );
        std::process::exit(1);
    }

    // Session-0 warning banner (Windows). The daemon runs fine in services
    // / SSH contexts for tools that don't touch the desktop, but every
    // window-driving tool (click, type_text, screenshot, get_window_state,
    // list_windows, launch_app for UWP) will fail or return empty when
    // invoked from this daemon. Surfacing this at startup saves users
    // hours of debugging tools that are working as designed.

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    #[cfg(unix)]
    let result = rt.block_on(run_serve_with_owner(
        registry.clone(),
        &socket_path,
        pid_file_path.as_deref(),
        profile,
        owner_pid,
    ));
    match result {
        Ok(ServeExit::OwnerExited) => std::process::exit(0),
        Ok(ServeExit::Shutdown) => {}
        Err(e) => {
            registry.remove_state_file();
            eprintln!("brigadier-cu serve error: {e}");
            std::process::exit(1);
        }
    }
}

/// `brigadier-cu stop` implementation.
pub fn run_stop_cmd(socket_path: &str) {
    if !is_daemon_listening(socket_path) {
        eprintln!("Brigadier Computer Use daemon is not running");
        std::process::exit(1);
    }

    let req = DaemonRequest {
        method: "shutdown".into(),
        name: None,
        args: None,
        session_id: None,
    };
    match send_request(socket_path, &req) {
        Ok(_) => {
            // Poll until daemon stops responding (up to 2 seconds).
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                // On Windows the pipe path isn't a filesystem file; probe liveness instead.
                let gone = if std::path::Path::new(socket_path).exists() {
                    false
                } else {
                    !is_daemon_listening(socket_path)
                };
                if gone {
                    // Swift's `stop` exits silently on success (no stdout
                    // line) — match that for byte-for-byte parity.
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    eprintln!("Brigadier Computer Use daemon did not release socket within 2s");
                    std::process::exit(1);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Err(e) => {
            eprintln!("stop: {e}");
            std::process::exit(1);
        }
    }
}

/// `brigadier-cu status` implementation.
pub fn run_status_cmd(socket_path: &str, pid_file_path: &str) {
    if is_daemon_listening(socket_path) {
        println!("Brigadier Computer Use daemon is running");
        println!("  socket: {socket_path}");
        if let Some(pid) = read_pid_file(pid_file_path) {
            println!("  pid: {pid}");
        } else {
            println!("  pid: unknown (no pid file)");
        }
    } else {
        eprintln!("Brigadier Computer Use daemon is not running");
        std::process::exit(1);
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod external_permission_flow_tests {
    use super::{
        clamp_external_permission_prompt, screen_capture_verification_response,
        validate_system_permission_request,
    };

    #[test]
    fn external_permission_flow_clamps_agent_prompt_requests() {
        let mut args = serde_json::json!({ "prompt": true });
        clamp_external_permission_prompt(true, "check_permissions", &mut args);
        assert_eq!(args["prompt"], serde_json::json!(false));

        let mut ordinary_tool_args = serde_json::json!({ "prompt": true });
        clamp_external_permission_prompt(true, "click", &mut ordinary_tool_args);
        assert_eq!(ordinary_tool_args["prompt"], serde_json::json!(true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn host_permission_request_rejects_unknown_permission_without_prompting() {
        let response = validate_system_permission_request(Some("camera"));
        assert!(!response.ok);
        assert_eq!(response.exit_code, Some(64));
        assert_eq!(
            response.error.as_deref(),
            Some("Unknown system permission: camera")
        );
    }

    #[test]
    fn host_screen_capture_verification_reports_a_timeout_as_unknown_with_the_error() {
        let timed_out = screen_capture_verification_response(Err(serde_json::json!({
            "code": "capture_timeout",
            "operation": "shareable_content",
            "elapsed_ms": 15000,
        })));
        assert!(timed_out.ok);
        let result = timed_out.result.unwrap();
        // null, never false: the host maps false to "not capturable" and asks
        // the user to re-grant a permission that is already granted.
        assert!(result["capturable"].is_null());
        assert_eq!(result["capture_error"]["code"], "capture_timeout");
        assert_eq!(result["capture_error"]["operation"], "shareable_content");
    }

    #[test]
    fn host_screen_capture_verification_reports_live_readiness_explicitly() {
        let ready = screen_capture_verification_response(Ok(true));
        assert!(ready.ok);
        assert_eq!(ready.result.unwrap()["capturable"], serde_json::json!(true));

        let unavailable = screen_capture_verification_response(Ok(false));
        assert!(unavailable.ok);
        assert_eq!(
            unavailable.result.unwrap()["capturable"],
            serde_json::json!(false)
        );
    }
}

#[cfg(test)]
mod socket_authentication_tests {
    use super::{
        attach_state_writer_identity, host_request_is_authorized, parse_request_with_token,
        parse_request_with_tokens, process_descends_from, socket_peer_requires_authorized_root,
        state_authentication_key, DaemonRequest, ProcessIdentity,
    };
    use std::collections::HashMap;

    fn list_request() -> DaemonRequest {
        DaemonRequest {
            method: "list".into(),
            name: None,
            args: None,
            session_id: None,
        }
    }

    #[test]
    fn authenticated_envelope_accepts_only_the_expected_token() {
        let request = list_request();
        let line = serde_json::json!({
            "auth_token": "secret-A1B2C3",
            "request": request,
        })
        .to_string();

        let parsed = parse_request_with_token(&line, Some("secret-A1B2C3"))
            .expect("matching token should authenticate");
        assert_eq!(parsed.method, "list");

        let rejected = parse_request_with_token(&line, Some("wrong-token"))
            .expect_err("mismatched token must be rejected");
        assert_eq!(rejected.exit_code, Some(77));
        assert_eq!(
            rejected.error.as_deref(),
            Some("Unauthorized daemon client")
        );
    }

    #[test]
    fn separate_host_capability_survives_agent_reparenting_without_leaking_host_authority() {
        let request = list_request();
        let host_line = serde_json::json!({
            "auth_token": "agent-secret-A1B2C3",
            "host_auth_token": "host-secret-D4E5F6",
            "request": request,
        })
        .to_string();
        let host = parse_request_with_tokens(
            &host_line,
            Some("agent-secret-A1B2C3"),
            Some("host-secret-D4E5F6"),
        )
        .expect("the host should retain its separate authority");
        assert!(host.host_authenticated);

        let agent_line = serde_json::json!({
            "auth_token": "agent-secret-A1B2C3",
            "request": list_request(),
        })
        .to_string();
        let agent = parse_request_with_tokens(
            &agent_line,
            Some("agent-secret-A1B2C3"),
            Some("host-secret-D4E5F6"),
        )
        .expect("an authenticated agent request should remain usable");
        assert!(!agent.host_authenticated);
        assert!(!socket_peer_requires_authorized_root(true, true));
        assert!(host_request_is_authorized(true, true, false));
        assert!(!host_request_is_authorized(true, false, true));
    }

    #[test]
    fn legacy_embedders_keep_process_ancestry_as_host_authority() {
        assert!(socket_peer_requires_authorized_root(true, false));
        assert!(host_request_is_authorized(false, false, true));
        assert!(!host_request_is_authorized(false, false, false));
    }

    #[test]
    fn state_authentication_configuration_accepts_only_32_byte_keys() {
        use base64::Engine as _;

        let encoded = base64::engine::general_purpose::STANDARD.encode([0x5a; 32]);
        assert_eq!(
            state_authentication_key(Some(&serde_json::json!({
                "key_base64": encoded
            }))),
            Some(vec![0x5a; 32])
        );
        assert!(state_authentication_key(Some(&serde_json::json!({
            "key_base64": base64::engine::general_purpose::STANDARD.encode([0x5a; 31])
        })))
        .is_none());
        assert!(state_authentication_key(Some(&serde_json::json!({
            "key_base64": "not-base64"
        })))
        .is_none());
    }

    #[test]
    fn configured_authentication_rejects_legacy_and_missing_requests() {
        let legacy = serde_json::to_string(&list_request()).expect("serialize request");
        let rejected = parse_request_with_token(&legacy, Some("secret-A1B2C3"))
            .expect_err("legacy request must not bypass configured auth");
        assert_eq!(rejected.exit_code, Some(77));

        let missing_request = r#"{"auth_token":"secret-A1B2C3"}"#;
        let rejected = parse_request_with_token(missing_request, Some("secret-A1B2C3"))
            .expect_err("authenticated envelope still requires a request");
        assert_eq!(rejected.exit_code, Some(65));
    }

    #[test]
    fn authentication_remains_opt_in_for_standalone_driver_clients() {
        let legacy = serde_json::to_string(&list_request()).expect("serialize request");
        let parsed = parse_request_with_token(&legacy, None)
            .expect("legacy request should work when auth is not configured");
        assert_eq!(parsed.method, "list");
    }

    #[test]
    fn authenticated_peer_pid_replaces_untrusted_state_writer_identity() {
        let mut args = serde_json::json!({
            "pid": 42,
            "_state_writer_pid": 999_999,
        });

        attach_state_writer_identity(
            &mut args,
            Some(ProcessIdentity {
                process_id: 4_321,
                parent_process_id: 4_000,
                start_seconds: 1_700_000_000,
                start_microseconds: 123_456,
            }),
        );
        assert_eq!(args["_state_writer_pid"], serde_json::json!(4_321));
        assert_eq!(
            args["_state_writer_start_seconds"],
            serde_json::json!(1_700_000_000)
        );
        assert_eq!(
            args["_state_writer_start_microseconds"],
            serde_json::json!(123_456)
        );

        attach_state_writer_identity(&mut args, None);
        assert!(args.get("_state_writer_pid").is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn authenticated_peer_generation_replaces_untrusted_state_writer_identity() {
        let mut args = serde_json::json!({
            "pid": 42,
            "_state_writer_pid": 999_999,
            "_state_writer_start_seconds": 999_999,
            "_state_writer_start_microseconds": 999_999,
        });

        let identity =
            super::macos_process_identity(std::process::id()).expect("current process identity");
        attach_state_writer_identity(&mut args, Some(identity));

        assert_eq!(
            args["_state_writer_pid"],
            serde_json::json!(identity.process_id)
        );
        assert_eq!(
            args["_state_writer_start_seconds"],
            serde_json::json!(identity.start_seconds)
        );
        assert_eq!(
            args["_state_writer_start_microseconds"],
            serde_json::json!(identity.start_microseconds)
        );

        attach_state_writer_identity(&mut args, None);
        assert!(args.get("_state_writer_pid").is_none());
        assert!(args.get("_state_writer_start_seconds").is_none());
        assert!(args.get("_state_writer_start_microseconds").is_none());
    }

    #[test]
    fn process_ancestry_accepts_only_the_configured_root_and_descendants() {
        let parents = HashMap::from([(400, 300), (300, 200), (200, 1), (500, 1)]);
        let parent = |pid| parents.get(&pid).copied();

        assert!(process_descends_from(400, 300, parent));
        assert!(process_descends_from(300, 300, parent));
        assert!(!process_descends_from(400, 500, parent));
        assert!(!process_descends_from(500, 300, parent));
        assert!(!process_descends_from(400, 1, parent));
    }

    #[test]
    fn process_ancestry_fails_closed_on_cycles_and_missing_metadata() {
        let cycle = HashMap::from([(400, 300), (300, 400)]);
        let parent = |pid| cycle.get(&pid).copied();
        assert!(!process_descends_from(400, 200, parent));
        assert!(!process_descends_from(400, 200, |_| None));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn macos_kernel_peer_pid_enforces_the_process_root() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let socket = directory.path().join("peer.sock");
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind test socket");
        let client = tokio::net::UnixStream::connect(&socket);
        let accepted = listener.accept();
        let (client, accepted) = tokio::join!(client, accepted);
        let _client = client.expect("connect test client");
        let (server, _) = accepted.expect("accept test client");

        let peer_process_id = super::macos_peer_process_id(&server);
        assert_eq!(peer_process_id, Some(std::process::id()));
        assert_eq!(
            peer_process_id.and_then(super::macos_process_identity),
            super::macos_process_identity(std::process::id())
        );
        let peer_identity = peer_process_id.and_then(super::macos_process_identity);
        let root_identity =
            super::macos_process_identity(std::process::id()).expect("root identity");
        assert!(super::macos_process_is_authorized(
            peer_identity,
            root_identity
        ));
        assert!(!super::macos_process_is_authorized(
            peer_identity,
            ProcessIdentity {
                start_seconds: root_identity.start_seconds.saturating_sub(1),
                ..root_identity
            }
        ));
    }
}

#[cfg(all(test, unix))]
mod gate_tests {
    //! Ended-session resurrection guard wired into the `call` dispatch.
    //!
    //! Closes PR #1779's gap: `is_session_ended()` was dead code, so an
    //! in-flight per-call request landing AFTER `session_end` fired would
    //! re-create session-owned metadata (cursor registry / config override)
    //! the reaper already passed. The gate REJECTS a `call` carrying an ended
    //! session id LOUDLY (isError) — a silent benign-ok was a trap that looked
    //! like success while doing nothing. The session-lifecycle tools are exempt:
    //! a `start_session` re-declare REVIVES the id so its subsequent actions run
    //! again. Live and anonymous calls always pass through.

    use super::{run_serve, secure_runtime_directory, send_request, DaemonProfile, DaemonRequest};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use cu_core::protocol::ToolResult;
    use cu_core::tool::{Tool, ToolDef, ToolRegistry};
    use serde_json::Value;

    static PROBE_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

    struct ProbeTool {
        def: ToolDef,
    }

    impl ProbeTool {
        fn new() -> Self {
            Self {
                def: ToolDef {
                    name: "probe".into(),
                    description: "test probe; bumps a shared invocation counter".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    read_only: false,
                    destructive: false,
                    idempotent: true,
                    open_world: false,
                },
            }
        }
    }

    #[tokio::test]
    async fn daemon_peer_with_a_foreign_uid_is_refused() {
        let root = tempfile::tempdir().expect("temp root");
        let socket = root.path().join("peer-uid.sock");
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind test socket");
        let _client = tokio::net::UnixStream::connect(&socket)
            .await
            .expect("connect test client");
        let (stream, _) = listener.accept().await.expect("accept test client");

        let effective_uid = unsafe { libc::geteuid() };
        super::check_peer_uid(&stream, effective_uid)
            .expect("a peer with our effective uid is accepted");

        let foreign_uid = effective_uid.wrapping_add(1);
        let refusal = super::check_peer_uid(&stream, foreign_uid)
            .expect_err("a peer whose uid differs from the expected uid is refused");
        assert!(
            refusal.contains(&format!("uid {effective_uid}"))
                && refusal.contains(&format!("expected uid {foreign_uid}")),
            "refusal names both uids: {refusal}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn socket_directory_refuses_an_ancestor_writable_by_other_users() {
        let root = tempfile::tempdir().expect("temp root");
        // A grandparent that another user can write to, without the sticky
        // bit, lets that user rename our private scope directory between the
        // check and the bind.
        let grandparent = root.path().join("shared-grandparent");
        std::fs::create_dir(&grandparent).expect("create grandparent");
        std::fs::set_permissions(&grandparent, std::fs::Permissions::from_mode(0o777))
            .expect("make grandparent writable by other users");
        let scope = grandparent.join("scope");
        std::fs::create_dir(&scope).expect("create scope");
        std::fs::set_permissions(&scope, std::fs::Permissions::from_mode(0o700))
            .expect("make scope private");
        let socket = scope.join("driver.sock");

        let registry = Arc::new(ToolRegistry::new());
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_serve(
                registry,
                socket.to_str().expect("socket path"),
                None,
                DaemonProfile::Native,
            ),
        )
        .await
        .expect("run_serve must fail closed instead of serving");
        let error = result.expect_err("an unsafe ancestor must fail closed");
        assert!(
            error.to_string().contains("writable by other users"),
            "unexpected error: {error}"
        );
        assert!(
            !socket.exists(),
            "no socket is bound under an unsafe ancestor"
        );
    }

    #[test]
    fn socket_directory_refuses_a_group_writable_ancestor() {
        let root = tempfile::tempdir().expect("temp root");
        // Our own ancestor, writable by its group and not sticky: a group
        // member could rename the scope directory below it.
        let grandparent = root.path().join("group-grandparent");
        std::fs::create_dir(&grandparent).expect("create grandparent");
        std::fs::set_permissions(&grandparent, std::fs::Permissions::from_mode(0o770))
            .expect("make grandparent group-writable");
        let scope = grandparent.join("scope");
        std::fs::create_dir(&scope).expect("create scope");
        std::fs::set_permissions(&scope, std::fs::Permissions::from_mode(0o700))
            .expect("make scope private");

        let error = super::open_private_runtime_directory(&scope, true)
            .expect_err("a group-writable ancestor must fail closed");
        assert!(
            error.to_string().contains("writable by other users"),
            "unexpected error: {error}"
        );

        std::fs::set_permissions(&grandparent, std::fs::Permissions::from_mode(0o1770))
            .expect("make grandparent sticky");
        super::open_private_runtime_directory(&scope, true)
            .expect("a sticky group-writable ancestor is accepted");
    }

    #[test]
    fn socket_binds_in_the_opened_directory_after_the_path_is_swapped() {
        let root = tempfile::tempdir().expect("temp root");
        let scope = root.path().join("scope");
        std::fs::create_dir(&scope).expect("create scope");
        std::fs::set_permissions(&scope, std::fs::Permissions::from_mode(0o700))
            .expect("make scope private");

        let directory = super::open_private_runtime_directory(&scope, false).expect("open scope");

        // Simulate the race: after the check, the path now names a different
        // directory, and our directory lives somewhere else.
        let moved = root.path().join("moved-scope");
        std::fs::rename(&scope, &moved).expect("move scope away");
        std::fs::create_dir(&scope).expect("create replacement directory");

        let name = std::ffi::OsStr::new("driver.sock");
        let listener = super::bind_socket_in_directory(&directory, name, &scope.join(name))
            .expect("bind through the opened directory");

        assert!(
            moved.join(name).exists(),
            "the socket is bound inside the directory that was checked"
        );
        assert!(
            !scope.join(name).exists(),
            "the socket must not follow the swapped path"
        );
        let mode = std::fs::metadata(moved.join(name))
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        drop(listener);
    }

    #[test]
    fn runtime_directory_is_private_and_rejects_symlinks() {
        let root = tempfile::tempdir().expect("temp root");
        let runtime = root.path().join("runtime");
        std::fs::create_dir(&runtime).expect("create runtime");
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o755))
            .expect("make legacy mode");

        secure_runtime_directory(&runtime, true).expect("harden runtime");
        let mode = std::fs::metadata(&runtime)
            .expect("runtime metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);

        let link = root.path().join("runtime-link");
        std::os::unix::fs::symlink(&runtime, &link).expect("create symlink");
        assert!(
            secure_runtime_directory(&link, true).is_err(),
            "runtime directory must not follow a symlink"
        );
    }

    #[tokio::test]
    async fn custom_socket_rejects_unsafe_directory_and_non_socket_path() {
        let root = tempfile::tempdir().expect("temp root");
        let unsafe_dir = root.path().join("shared");
        std::fs::create_dir(&unsafe_dir).expect("create shared directory");
        std::fs::set_permissions(&unsafe_dir, std::fs::Permissions::from_mode(0o777))
            .expect("make directory unsafe");

        let registry = Arc::new(ToolRegistry::new());
        let unsafe_socket = unsafe_dir.join("driver.sock");
        let error = run_serve(
            registry.clone(),
            unsafe_socket.to_str().expect("unsafe socket path"),
            None,
            DaemonProfile::Native,
        )
        .await
        .expect_err("unsafe custom socket directory must fail closed");
        assert!(error.to_string().contains("unsafe permissions"));

        let safe_dir = root.path().join("private");
        std::fs::create_dir(&safe_dir).expect("create private directory");
        std::fs::set_permissions(&safe_dir, std::fs::Permissions::from_mode(0o700))
            .expect("make directory private");
        let regular_file = safe_dir.join("driver.sock");
        std::fs::write(&regular_file, b"do not delete").expect("create regular file");

        let error = run_serve(
            registry,
            regular_file.to_str().expect("regular file path"),
            None,
            DaemonProfile::Native,
        )
        .await
        .expect_err("non-socket path must not be replaced");
        assert!(error.to_string().contains("non-socket"));
        assert_eq!(
            std::fs::read(&regular_file).expect("regular file preserved"),
            b"do not delete"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn daemon_socket_and_pid_file_are_private() {
        let root = tempfile::tempdir().expect("temp root");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("make temp root private");
        let socket = root.path().join("driver.sock");
        let pid_file = root.path().join("driver.pid");
        let socket_for_server = socket.clone();
        let pid_for_server = pid_file.clone();
        let registry = Arc::new(ToolRegistry::new());
        let server = tokio::spawn(async move {
            run_serve(
                registry,
                socket_for_server.to_str().expect("socket path"),
                Some(pid_for_server.to_str().expect("pid path")),
                DaemonProfile::Native,
            )
            .await
        });

        for _ in 0..100 {
            if socket.exists() && pid_file.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        for path in [&socket, &pid_file] {
            let mode = std::fs::metadata(path)
                .unwrap_or_else(|error| panic!("{} metadata: {error}", path.display()))
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{} must be private", path.display());
        }

        #[cfg(target_os = "macos")]
        {
            let status_request = DaemonRequest {
                method: "permissions_status".into(),
                name: None,
                args: None,
                session_id: None,
            };
            let socket_for_status = socket.to_string_lossy().into_owned();
            let response = tokio::task::spawn_blocking(move || {
                send_request(&socket_for_status, &status_request)
            })
            .await
            .expect("permission status task")
            .expect("permission status response");
            assert!(response.ok);
            let status = response.result.expect("permission status payload");
            assert_eq!(status["profile"], "native");
            assert_eq!(status["source"]["attribution"], "caller");
            assert!(status["source"]["instance"].as_str().is_some());
        }

        let shutdown = DaemonRequest {
            method: "shutdown".into(),
            name: None,
            args: None,
            session_id: None,
        };
        let socket_for_client = socket.to_string_lossy().into_owned();
        let response =
            tokio::task::spawn_blocking(move || send_request(&socket_for_client, &shutdown))
                .await
                .expect("shutdown task")
                .expect("shutdown response");
        assert!(response.ok);
        server.await.expect("server task").expect("server result");
    }

    #[async_trait]
    impl Tool for ProbeTool {
        fn def(&self) -> &ToolDef {
            &self.def
        }
        async fn invoke(&self, _args: Value) -> ToolResult {
            PROBE_INVOCATIONS.fetch_add(1, Ordering::SeqCst);
            ToolResult::text("probe ran")
        }
    }

    fn call_req(sid: Option<&str>) -> DaemonRequest {
        DaemonRequest {
            method: "call".into(),
            name: Some("probe".into()),
            args: Some(serde_json::json!({})),
            session_id: sid.map(|s| s.to_owned()),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ended_session_call_is_gated_live_and_anon_pass() {
        PROBE_INVOCATIONS.store(0, Ordering::SeqCst);

        let mut reg = ToolRegistry::new();
        reg.register(Box::new(ProbeTool::new()));
        // The real start_session tool so we can prove an explicit re-declare
        // REVIVES an ended id end-to-end through the daemon boundary.
        reg.register(Box::new(cu_core::session_tools::StartSessionTool));
        let registry = Arc::new(reg);

        // Unique private temp socket, never the default socket or app daemon.
        let socket_root = tempfile::tempdir().expect("socket temp root");
        std::fs::set_permissions(socket_root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("make socket temp root private");
        let socket = socket_root
            .path()
            .join("driver.sock")
            .to_string_lossy()
            .into_owned();
        let socket_for_server = socket.clone();
        let reg_for_server = registry.clone();
        let server = tokio::spawn(async move {
            let _ = run_serve(
                reg_for_server,
                &socket_for_server,
                None,
                DaemonProfile::Native,
            )
            .await;
        });

        // Wait for the daemon to bind.
        for _ in 0..100 {
            if std::path::Path::new(&socket).exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let sid = "gate-test-session-A1B2C3";

        // 1. LIVE session call → tool runs.
        let socket1 = socket.clone();
        let s1 = sid.to_owned();
        let resp =
            tokio::task::spawn_blocking(move || send_request(&socket1, &call_req(Some(&s1))))
                .await
                .unwrap()
                .expect("live call response");
        assert!(resp.ok, "live-session call should succeed");
        assert_eq!(
            PROBE_INVOCATIONS.load(Ordering::SeqCst),
            1,
            "live-session call must invoke the tool"
        );

        // 2. End the session (mirrors the control-conn EOF reaper path).
        let socket2 = socket.clone();
        let end = DaemonRequest {
            method: "session_end".into(),
            name: None,
            args: None,
            session_id: Some(sid.to_owned()),
        };
        let resp = tokio::task::spawn_blocking(move || send_request(&socket2, &end))
            .await
            .unwrap()
            .expect("session_end response");
        assert!(resp.ok, "session_end should ack ok");

        // 3. ENDED session call carrying the same sid → REJECTED LOUDLY. The
        //    tool must NOT run, and the caller must see a failure (not a phantom
        //    success that silently does nothing).
        let socket3 = socket.clone();
        let s3 = sid.to_owned();
        let resp =
            tokio::task::spawn_blocking(move || send_request(&socket3, &call_req(Some(&s3))))
                .await
                .unwrap()
                .expect("ended call response");
        assert!(
            !resp.ok,
            "ended-session call must be rejected loudly (not ok)"
        );
        assert!(
            resp.error.as_deref().unwrap_or("").contains("has ended"),
            "rejection must explain the session ended; got {:?}",
            resp.error
        );
        assert_eq!(
            PROBE_INVOCATIONS.load(Ordering::SeqCst),
            1,
            "rejected ended-session call must not invoke the tool — resurrection closed"
        );

        // 3b. Explicit re-declare via start_session REVIVES the id.
        let socket3b = socket.clone();
        let s3b = sid.to_owned();
        let start = DaemonRequest {
            method: "call".into(),
            name: Some("start_session".into()),
            args: Some(serde_json::json!({})),
            session_id: Some(s3b),
        };
        let resp = tokio::task::spawn_blocking(move || send_request(&socket3b, &start))
            .await
            .unwrap()
            .expect("start_session response");
        assert!(
            resp.ok,
            "start_session must run even for an ended id (lifecycle-exempt)"
        );

        // 3c. A call on the revived id now RUNS again.
        let socket3c = socket.clone();
        let s3c = sid.to_owned();
        let resp =
            tokio::task::spawn_blocking(move || send_request(&socket3c, &call_req(Some(&s3c))))
                .await
                .unwrap()
                .expect("revived call response");
        assert!(resp.ok, "call on a revived session should succeed");
        assert_eq!(
            PROBE_INVOCATIONS.load(Ordering::SeqCst),
            2,
            "revived-session call must invoke the tool again"
        );

        // 4. Anonymous call (no session id) still passes — no false positive.
        let socket4 = socket.clone();
        let resp = tokio::task::spawn_blocking(move || send_request(&socket4, &call_req(None)))
            .await
            .unwrap()
            .expect("anon call response");
        assert!(resp.ok, "anonymous call should succeed");
        assert_eq!(
            PROBE_INVOCATIONS.load(Ordering::SeqCst),
            3,
            "anonymous (no session id) call must still invoke the tool"
        );

        // Tear down the daemon.
        let socket5 = socket.clone();
        let shutdown = DaemonRequest {
            method: "shutdown".into(),
            name: None,
            args: None,
            session_id: None,
        };
        let _ = tokio::task::spawn_blocking(move || send_request(&socket5, &shutdown)).await;
        let _ = server.await;
        let _ = std::fs::remove_file(&socket);
    }
}

#[cfg(all(test, unix))]
mod owner_pid_tests {
    //! `serve --owner-pid`: the daemon exits when its owning process exits
    //! (for example the host app crashes), and it removes its socket.

    use super::{run_serve_with_owner, DaemonProfile, ServeExit};
    use cu_core::tool::ToolRegistry;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::time::Duration;

    fn private_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("temp root");
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
            .expect("make temp root private");
        root
    }

    fn spawn_owner() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep owner")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_exits_and_removes_its_socket_when_the_owner_exits() {
        let root = private_root();
        let socket = root.path().join("owned.sock");
        let pid_file = root.path().join("owned.pid");
        let mut owner = spawn_owner();
        let owner_pid = owner.id() as i32;

        let socket_for_server = socket.clone();
        let pid_for_server = pid_file.clone();
        let server = tokio::spawn(async move {
            run_serve_with_owner(
                Arc::new(ToolRegistry::new()),
                socket_for_server.to_str().expect("socket path"),
                Some(pid_for_server.to_str().expect("pid path")),
                DaemonProfile::Native,
                Some(owner_pid),
            )
            .await
        });

        let bind_deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !socket.exists() {
            assert!(
                !server.is_finished(),
                "serve stopped before the owner exited"
            );
            assert!(
                std::time::Instant::now() < bind_deadline,
                "serve did not bind"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        owner.kill().expect("kill owner");
        owner.wait().expect("reap owner");

        let exit = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("serve must exit within 2 s after the owner exits")
            .expect("serve task")
            .expect("serve result");
        assert_eq!(exit, ServeExit::OwnerExited);
        assert!(!socket.exists(), "serve must remove its socket");
        assert!(!pid_file.exists(), "serve must remove its pid file");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_exits_at_once_when_the_owner_is_already_dead() {
        let root = private_root();
        let socket = root.path().join("orphan.sock");
        let mut owner = spawn_owner();
        let owner_pid = owner.id() as i32;
        owner.kill().expect("kill owner");
        owner.wait().expect("reap owner");

        let exit = tokio::time::timeout(
            Duration::from_secs(2),
            run_serve_with_owner(
                Arc::new(ToolRegistry::new()),
                socket.to_str().expect("socket path"),
                None,
                DaemonProfile::Native,
                Some(owner_pid),
            ),
        )
        .await
        .expect("serve must exit at once when the owner is already dead")
        .expect("serve result");
        assert_eq!(exit, ServeExit::OwnerExited);
        assert!(!socket.exists(), "serve must not leave a socket behind");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_without_an_owner_keeps_running() {
        let root = private_root();
        let socket = root.path().join("unowned.sock");
        let socket_for_server = socket.clone();
        let server = tokio::spawn(async move {
            run_serve_with_owner(
                Arc::new(ToolRegistry::new()),
                socket_for_server.to_str().expect("socket path"),
                None,
                DaemonProfile::Native,
                None,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            !server.is_finished(),
            "serve without --owner-pid must keep running"
        );
        server.abort();
        let _ = std::fs::remove_file(&socket);
    }
}

#[cfg(test)]
mod session_boundary_tests {
    use super::{apply_session_identity, DaemonProfile};
    use serde_json::json;

    #[test]
    fn explicit_session_becomes_session_id_and_is_returned() {
        let mut args = json!({ "x": 1, "session": "research-1" });
        let eff = apply_session_identity(&mut args, &None);
        assert_eq!(eff.as_deref(), Some("research-1"));
        assert_eq!(args["_session_id"], "research-1");
    }

    #[test]
    fn no_session_falls_back_to_minted_for_session_id_only() {
        // The minted per-connection id drives `_session_id` (recording / config
        // lifecycle) but there is NO explicit `session`, so the cursor resolver
        // — which reads `session`/`cursor_id`, not `_session_id` — sees nothing.
        let mut args = json!({ "x": 1 });
        let eff = apply_session_identity(&mut args, &Some("mcp-123".to_owned()));
        assert_eq!(args["_session_id"], "mcp-123");
        assert_eq!(eff.as_deref(), Some("mcp-123"));
        assert!(args.get("session").is_none());
    }

    #[test]
    fn anonymous_when_no_session_and_no_minted() {
        let mut args = json!({ "x": 1 });
        let eff = apply_session_identity(&mut args, &None);
        assert!(eff.is_none());
        assert!(args.get("_session_id").is_none());
    }

    #[test]
    fn caller_set_session_id_is_not_overwritten_by_minted() {
        let mut args = json!({ "_session_id": "caller-set" });
        let eff = apply_session_identity(&mut args, &Some("mcp-999".to_owned()));
        assert_eq!(args["_session_id"], "caller-set");
        assert_eq!(eff.as_deref(), Some("caller-set"));
    }

    #[test]
    fn daemon_profiles_have_stable_wire_names() {
        assert_eq!(
            serde_json::to_value(DaemonProfile::Native).unwrap(),
            json!("native")
        );
    }
}
