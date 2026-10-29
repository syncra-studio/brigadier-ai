//! Windows task-browser ownership. No inbound attach, user-profile setup or
//! recursive deletion. Only internally created, lifecycle-bound profiles get
//! automatically generated DevTools arguments.
use crate::browser_platform::WindowsBrowserPlatform;
use anyhow::{anyhow, bail, Context, Result};
use cua_driver_core::browser::types::{OwnedEndpoint, ProcessFingerprint};
use cua_driver_core::browser::{parse_devtools_active_port, BrowserPlatform};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DirectoryId {
    volume: u32,
    index: u64,
    created: u64,
}

fn directory_id(file: &File) -> Result<DirectoryId> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info)?;
    }
    if info.dwFileAttributes & 0x400 != 0 {
        bail!("task profile directory is a reparse point");
    }
    Ok(DirectoryId {
        volume: info.dwVolumeSerialNumber,
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        created: (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
            | u64::from(info.ftCreationTime.dwLowDateTime),
    })
}

fn reject_reparse_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if std::fs::symlink_metadata(ancestor)?.file_attributes() & 0x400 != 0 {
            bail!("task browser profile ancestry contains a reparse point");
        }
    }
    Ok(())
}

fn nonce() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow!("ownership nonce unavailable: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

static RUNTIME: OnceLock<String> = OnceLock::new();
static PROFILES: Mutex<Vec<Arc<Profile>>> = Mutex::new(Vec::new());

pub(super) fn lifecycle(args: &Value) -> Result<String> {
    if let Some(key) = cua_driver_core::tool_args::session_key(args) {
        return Ok(key.into());
    }
    if RUNTIME.get().is_none() {
        let _ = RUNTIME.set(nonce()?);
    }
    Ok(format!("runtime-{}", RUNTIME.get().unwrap()))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Marker {
    schema: String,
    nonce: String,
    lifecycle: String,
    creator_pid: u32,
    executable: String,
    directory: DirectoryId,
    process: Option<ProcessFingerprint>,
    endpoint: Option<String>,
    retired: bool,
}

pub(super) struct Profile {
    pub(super) path: PathBuf,
    pub(super) executable: String,
    directory: Mutex<Option<File>>,
    marker_file: Mutex<Option<File>>,
    record: Mutex<Marker>,
    watching: std::sync::atomic::AtomicBool,
}

impl Profile {
    pub(super) fn known_pid(&self) -> Option<u32> {
        self.record
            .lock()
            .unwrap()
            .process
            .as_ref()
            .and_then(|p| u32::try_from(p.pid).ok())
    }
    fn verify(&self) -> Result<Marker> {
        let expected = self.record.lock().unwrap().clone();
        if expected.retired {
            bail!("the task-browser lifecycle has ended");
        }
        reject_reparse_ancestors(&self.path)?;
        let directory = self.directory.lock().unwrap();
        if directory_id(
            directory
                .as_ref()
                .context("task lifecycle directory handle ended")?,
        )? != expected.directory
        {
            bail!("task profile directory identity changed");
        }
        let current = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0)
            .share_mode(3)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&self.path)?;
        if directory_id(&current)? != expected.directory {
            bail!("task profile path was replaced");
        }
        let mut file = self.marker_file.lock().unwrap();
        let file = file
            .as_mut()
            .context("task lifecycle marker handle ended")?;
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        (&mut *file).take(65537).read_to_end(&mut bytes)?;
        if bytes.len() > 65536 || serde_json::from_slice::<Marker>(&bytes)? != expected {
            bail!("task-browser lifecycle/ownership marker changed");
        }
        Ok(expected)
    }

    fn persist(&self, next: Marker) -> Result<()> {
        let bytes = serde_json::to_vec(&next)?;
        let mut file = self.marker_file.lock().unwrap();
        let file = file
            .as_mut()
            .context("task lifecycle marker handle ended")?;
        file.seek(SeekFrom::Start(0))?;
        file.set_len(0)?;
        file.write_all(&bytes)?;
        file.sync_data()?;
        *self.record.lock().unwrap() = next;
        Ok(())
    }

    pub(super) fn arguments(&self) -> Vec<String> {
        vec![
            super::browser_target::quote_argument(&format!(
                "--user-data-dir={}",
                self.path.display()
            )),
            "--remote-debugging-address=127.0.0.1".into(),
            "--remote-debugging-port=0".into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-background-mode".into(),
            // No startup window means no startup activation request; pages
            // open through the proven endpoint as inactive windows instead.
            "--no-startup-window".into(),
        ]
    }

    /// Open the requested pages as background targets through this profile's
    /// proven endpoint: the first in a new, inactive, off-screen window, the
    /// rest as background tabs. Chromium never requests activation for them.
    pub(super) async fn open_pages(&self, urls: &[String]) -> Result<()> {
        let record = self.verify()?;
        let ws = record
            .endpoint
            .context("the task-browser endpoint has not been proven")?;
        let conn = cua_driver_core::browser::cdp_ws::CdpConnection::connect(&ws).await?;
        let (x, y) = offscreen_origin();
        let (width, height) = default_window_size();
        let first = urls
            .first()
            .map(|url| page_url(url))
            .unwrap_or_else(|| "about:blank".into());
        conn.call(
            None,
            "Target.createTarget",
            json!({"url": first, "newWindow": true, "background": true,
                   "left": x, "top": y, "width": width, "height": height}),
        )
        .await?;
        for url in urls.iter().skip(1) {
            conn.call(
                None,
                "Target.createTarget",
                json!({"url": page_url(url), "background": true}),
            )
            .await?;
        }
        Ok(())
    }

    pub(super) async fn confirm(&self, launched_pid: u32) -> Result<ProcessFingerprint> {
        let record = self.verify()?;
        let platform = WindowsBrowserPlatform::default();
        let root = record
            .process
            .as_ref()
            .map(|p| p.pid)
            .unwrap_or(i64::from(launched_pid));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(6);
        loop {
            let attempt: Result<_> = async {
                let text = std::fs::read_to_string(self.path.join("DevToolsActivePort"))?;
                let (port, ws_path) = parse_devtools_active_port(&text)
                    .context("invalid private-profile endpoint file")?;
                let expected = format!("ws://127.0.0.1:{port}{ws_path}");
                let endpoint = if record.process.is_some() {
                    platform
                        .discover_owned_endpoint(root)
                        .await
                        .map_err(|e| anyhow!(e.message))?
                } else {
                    platform
                        .discover_spawned_endpoint(root, &expected)
                        .await
                        .map_err(|e| anyhow!(e.message))?
                }
                .context("private endpoint is not process-owned")?;
                if endpoint.ws_url != expected {
                    bail!("profile endpoint differs from the process-owned endpoint");
                }
                let pid = endpoint
                    .ownership
                    .listener_pid
                    .unwrap_or(endpoint.ownership.owner_pid);
                let process = platform
                    .process_fingerprint(pid)
                    .await
                    .map_err(|e| anyhow!(e.message))?;
                if process.start_time.is_none()
                    || process
                        .executable
                        .as_deref()
                        .and_then(crate::browser_installation_path::plain_windows_path)
                        .is_none_or(|exe| !exe.eq_ignore_ascii_case(&self.executable))
                {
                    bail!("private browser executable/lifetime proof failed");
                }
                if record.process.as_ref().is_some_and(|old| old != &process) {
                    bail!("private browser process was replaced");
                }
                Ok((process, endpoint))
            }
            .await;
            match attempt {
                Ok((process, endpoint)) => {
                    let mut next = record;
                    next.process = Some(process.clone());
                    next.endpoint = Some(endpoint.ws_url);
                    self.persist(next)?;
                    return Ok(process);
                }
                Err(error) if std::time::Instant::now() >= deadline => return Err(error),
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            }
        }
    }
}

/// Caller profile-selection arguments opt out. Never append debug flags to a
/// caller-selected directory, even if its name resembles an owned profile.
pub(super) fn explicit_profile(target: &str, args: &[String]) -> bool {
    let all = std::iter::once(target).chain(args.iter().map(String::as_str));
    all.flat_map(|s| s.split(|c: char| c.is_whitespace() || c == '"' || c == '\''))
        .any(|s| {
            s.starts_with("--user-data-dir")
                || s.starts_with("--profile-directory")
                || s == "--guest"
                || s == "--incognito"
        })
}

pub(super) fn prepare(
    target: &str,
    args: &[String],
    key: &str,
    new_instance: bool,
) -> Result<Option<Arc<Profile>>> {
    if explicit_profile(target, args) {
        return Ok(None);
    }
    let (file, _) = super::impl_::split_launchable_target(target);
    let Some(executable) = WindowsBrowserPlatform::default()
        .task_browser_executable(&file)
        .map_err(|e| anyhow!(e.message))?
    else {
        return Ok(None);
    };
    if !new_instance {
        for profile in PROFILES.lock().unwrap().iter().rev() {
            let record = profile.record.lock().unwrap().clone();
            if record.lifecycle == key
                && !record.retired
                && profile.executable.eq_ignore_ascii_case(&executable)
            {
                // A live process is re-proven asynchronously before reuse below.
                let living = record.process.as_ref().is_some_and(|process| {
                    crate::browser_platform::process_identity(process.pid as u32).is_ok_and(
                        |(start, exe)| {
                            process.start_time == Some(start)
                                && exe
                                    .as_deref()
                                    .and_then(crate::browser_installation_path::plain_windows_path)
                                    .is_some_and(|exe| {
                                        exe.eq_ignore_ascii_case(&profile.executable)
                                    })
                        },
                    )
                });
                if living && profile.verify().is_ok() {
                    return Ok(Some(profile.clone()));
                }
            }
        }
    }
    // The driver's own profile root (set by the host), else per-user state.
    let root = std::env::var_os("CUA_DRIVER_BROWSER_PROFILE_ROOT")
        .filter(|root| !root.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("LOCALAPPDATA").map(|local| {
                PathBuf::from(local)
                    .join("CuaDriver")
                    .join("BrowserProfiles")
            })
        })
        .context("no driver-owned browser profile directory is configured")?
        .join("task-browsers");
    std::fs::create_dir_all(&root)?;
    reject_reparse_ancestors(&root)?;
    let id = nonce()?;
    let path = root.join(format!("task-{id}"));
    std::fs::create_dir(&path)?;
    let directory = OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode(3)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(&path)?;
    let marker = Marker {
        schema: "brigadier-task-browser-v1".into(),
        nonce: id,
        lifecycle: key.into(),
        creator_pid: std::process::id(),
        executable: executable.clone(),
        directory: directory_id(&directory)?,
        process: None,
        endpoint: None,
        retired: false,
    };
    let marker_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(1)
        .open(path.join("brigadier-task-owner.json"))?;
    let profile = Arc::new(Profile {
        path,
        executable,
        directory: Mutex::new(Some(directory)),
        marker_file: Mutex::new(Some(marker_file)),
        record: Mutex::new(marker.clone()),
        watching: std::sync::atomic::AtomicBool::new(false),
    });
    profile.persist(marker)?;
    PROFILES.lock().unwrap().push(profile.clone());
    // Retain an exact audit manifest; no failure/drop/session path deletes it.
    if let Some(home) = std::env::var_os("CUA_DRIVER_STATE_DIR") {
        let mut audit = OpenOptions::new()
            .create(true)
            .append(true)
            .open(PathBuf::from(home).join("task-browser-profiles.jsonl"))?;
        writeln!(
            audit,
            "{}",
            json!({"profile":profile.path,"owner":profile.record.lock().unwrap().clone()})
        )?;
    }
    Ok(Some(profile))
}

pub(super) fn retire(key: &str) {
    for profile in PROFILES.lock().unwrap().iter() {
        let record = profile.record.lock().unwrap().clone();
        if record.lifecycle == key {
            let mut next = record;
            next.retired = true;
            let _ = profile.persist(next);
            profile.marker_file.lock().unwrap().take();
            profile.directory.lock().unwrap().take();
        }
    }
}

/// Known task profiles cannot be attached by another lifecycle. Ordinary
/// pre-existing addressed browsers are not set up or redirected here.
pub(super) async fn endpoint_at_use(pid: u32, key: &str) -> Result<Option<OwnedEndpoint>> {
    let profiles = PROFILES.lock().unwrap().clone();
    for profile in profiles {
        let record = profile.record.lock().unwrap().clone();
        if record
            .process
            .as_ref()
            .is_none_or(|p| p.pid != i64::from(pid))
        {
            continue;
        }
        if record.lifecycle != key || record.retired {
            bail!("the task browser belongs to a different or ended lifecycle");
        }
        profile.verify()?;
        let platform = WindowsBrowserPlatform::default();
        let current = platform
            .process_fingerprint(pid.into())
            .await
            .map_err(|e| anyhow!(e.message))?;
        if Some(&current) != record.process.as_ref() {
            bail!("the task browser process identity changed");
        }
        let endpoint = platform
            .discover_owned_endpoint(pid.into())
            .await
            .map_err(|e| anyhow!(e.message))?
            .context("task endpoint no longer owned")?;
        if Some(&endpoint.ws_url) != record.endpoint.as_ref() {
            bail!("task endpoint changed; no arbitrary attach attempted");
        }
        let text = std::fs::read_to_string(profile.path.join("DevToolsActivePort"))?;
        let (port, path) =
            parse_devtools_active_port(&text).context("private endpoint file no longer valid")?;
        if endpoint.ws_url != format!("ws://127.0.0.1:{port}{path}") {
            bail!("private-profile endpoint file changed");
        }
        profile.verify()?;
        return Ok(Some(endpoint));
    }
    Ok(None)
}

/// Off-screen origin for new task-browser windows: beyond every monitor, so
/// the new top-level window covers nothing until it is placed.
fn offscreen_origin() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    let (x, y, w, h) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    (x + w + 3000, y + h + 3000)
}

/// Command-line launches accepted local paths; CDP targets need URLs.
fn page_url(value: &str) -> String {
    let bytes = value.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');
    let unc = value.starts_with(r"\\") || value.starts_with("//");
    if !drive && !unc {
        return value.to_owned();
    }
    let path = value.replace('\\', "/");
    let mut url = String::from(if drive { "file:///" } else { "file:" });
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"/:-._~!$&'()*+,;=@".contains(&byte) {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

/// Caller window-geometry flags opt out of off-screen placement.
pub(super) fn offscreen_argument(caller_args: &[String]) -> Option<String> {
    let explicit = caller_args.iter().any(|arg| {
        [
            "--window-position",
            "--start-maximized",
            "--start-fullscreen",
            "--kiosk",
            "--app",
        ]
        .iter()
        .any(|flag| arg.contains(flag))
    });
    (!explicit).then(|| {
        let (x, y) = offscreen_origin();
        format!("--window-position={x},{y}")
    })
}

/// How the launch's new windows reached the screen.
#[derive(Default)]
pub(super) struct Placement {
    pub(super) placed: Vec<u64>,
    pub(super) behind_active: bool,
    pub(super) on_screen_before_placement: bool,
}

impl Placement {
    pub(super) fn note(&self) -> &'static str {
        if self.placed.is_empty() {
            " No new browser window was found to place; one may still be off-screen. Inspect list_windows."
        } else if self.on_screen_before_placement {
            " A new browser window appeared on screen before it could be placed; it was then moved behind your active window without being activated."
        } else if self.behind_active {
            " The browser window opened off-screen and was then placed directly behind your active window, without being activated or covering it."
        } else {
            " The browser window opened off-screen and was then placed at the back of the window stack, without being activated."
        }
    }
}

/// Move every new top-level window of the task browser on screen behind the
/// user's active window, one SetWindowPos each. Never activates, never moves
/// or restacks the user's window, and never touches a pre-existing window.
pub(super) async fn place_new_windows(
    pid: u32,
    preexisting: &std::collections::HashSet<u64>,
) -> Placement {
    let preexisting = preexisting.clone();
    tokio::task::spawn_blocking(move || {
        let mut placement = Placement::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut quiet_since = None;
        loop {
            let fresh: Vec<_> = crate::win32::list_windows_via_win32(Some(pid))
                .into_iter()
                .filter(|w| {
                    w.pid == pid
                        && !w.minimized
                        && is_browser_frame(w.hwnd)
                        && !preexisting.contains(&w.hwnd)
                        && !placement.placed.contains(&w.hwnd)
                })
                .collect();
            for window in &fresh {
                if let Some(behind) = place_one(window, &mut placement) {
                    placement.placed.push(window.hwnd);
                    placement.behind_active |= behind;
                }
            }
            let now = std::time::Instant::now();
            if !placement.placed.is_empty() && fresh.is_empty() {
                let since = *quiet_since.get_or_insert(now);
                if now.duration_since(since) >= std::time::Duration::from_millis(150) {
                    break;
                }
            } else {
                quiet_since = None;
            }
            if now >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        placement
    })
    .await
    .unwrap_or_default()
}

/// Returns `Some(true)` when placed directly behind the active window,
/// `Some(false)` when placed at the back of the stack.
fn place_one(window: &crate::win32::WindowInfo, placement: &mut Placement) -> Option<bool> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowThreadProcessId, SetWindowPos,
        GWL_EXSTYLE, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
        WS_EX_TOPMOST,
    };
    let hwnd = HWND(window.hwnd as *mut _);
    let (offscreen_x, offscreen_y) = offscreen_origin();
    let offscreen = window.x >= offscreen_x - 100 || window.y >= offscreen_y - 100;
    let active = unsafe { GetForegroundWindow() };
    let mut active_pid = 0;
    unsafe { GetWindowThreadProcessId(active, Some(&mut active_pid)) };
    let mut class = [0u16; 64];
    let len = unsafe { GetClassNameW(active, &mut class) }.max(0) as usize;
    let class = String::from_utf16_lossy(&class[..len]);
    let topmost = unsafe { GetWindowLongW(active, GWL_EXSTYLE) } as u32 & WS_EX_TOPMOST.0 != 0;
    // Behind the user's window when there is one; otherwise behind everything.
    let behind_active = !active.is_invalid()
        && active_pid != window.pid
        && !topmost
        && !matches!(
            class.as_str(),
            "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
        );
    let after = if behind_active { active } else { HWND_BOTTOM };
    let result = if offscreen {
        let monitor = unsafe { MonitorFromWindow(active, MONITOR_DEFAULTTOPRIMARY) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            return None;
        }
        let work = info.rcWork;
        let width = window.width.min(work.right - work.left - 32).max(1);
        let height = window.height.min(work.bottom - work.top - 32).max(1);
        unsafe {
            SetWindowPos(
                hwnd,
                after,
                work.left + 16,
                work.top + 16,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
        }
    } else {
        // Already on screen (the browser ignored the position): end any
        // covering now without moving it.
        placement.on_screen_before_placement = true;
        unsafe {
            SetWindowPos(
                hwnd,
                after,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOSIZE | SWP_NOMOVE,
            )
        }
    };
    result.ok().map(|_| behind_active)
}

/// A normal window size inside the primary monitor's work area.
fn default_window_size() -> (i32, i32) {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    };
    let mut work = RECT::default();
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut RECT as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    }
    .is_ok();
    if !ok {
        return (1280, 800);
    }
    (
        (work.right - work.left - 32).clamp(640, 1280),
        (work.bottom - work.top - 32).clamp(480, 900),
    )
}

/// Merge `feature` into the one `--disable-features=` switch (Chromium
/// honours only the last such switch), or add one.
pub(super) fn disable_feature(extra_args: &mut Vec<String>, feature: &str) {
    if let Some(index) = extra_args
        .iter()
        .rposition(|arg| arg.starts_with("--disable-features="))
    {
        if !extra_args[index]
            .trim_start_matches("--disable-features=")
            .split(',')
            .any(|existing| existing.trim() == feature)
        {
            extra_args[index] = format!("{},{feature}", extra_args[index]);
        }
    } else {
        extra_args.push(format!("--disable-features={feature}"));
    }
}

impl Profile {
    /// Edge implicitly signs a new profile into the Windows user's account,
    /// which would sync the user's own data into a task-controlled profile.
    pub(super) fn disabled_features(&self) -> &'static [&'static str] {
        let edge = Path::new(&self.executable)
            .file_stem()
            .is_some_and(|stem| stem.eq_ignore_ascii_case("msedge"));
        if edge {
            &["msImplicitSignin"]
        } else {
            &[]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_features_merge_into_the_single_switch() {
        let mut args = vec![
            "--disable-features=CalculateNativeWinOcclusion".to_owned(),
            "--new-window".to_owned(),
        ];
        disable_feature(&mut args, "msImplicitSignin");
        disable_feature(&mut args, "msImplicitSignin");
        assert_eq!(
            args[0],
            "--disable-features=CalculateNativeWinOcclusion,msImplicitSignin"
        );
        let mut none = vec!["--new-window".to_owned()];
        disable_feature(&mut none, "msImplicitSignin");
        assert_eq!(none[1], "--disable-features=msImplicitSignin");
    }

    #[test]
    fn local_paths_become_file_urls_and_urls_pass_through() {
        assert_eq!(
            page_url(r"C:\test dir\a#b.html"),
            "file:///C:/test%20dir/a%23b.html"
        );
        assert_eq!(
            page_url(r"\\server\share\x.html"),
            "file://server/share/x.html"
        );
        assert_eq!(
            page_url("https://example.com/?q=1"),
            "https://example.com/?q=1"
        );
    }

    #[test]
    fn caller_profile_and_geometry_flags_opt_out() {
        assert!(explicit_profile(
            "chrome",
            &[r"--user-data-dir=C:\x".into()]
        ));
        assert!(explicit_profile(
            "chrome",
            &["--profile-directory=\"Profile 2\"".into()]
        ));
        assert!(!explicit_profile("chrome", &["--new-window".into()]));
        assert!(offscreen_argument(&["--start-maximized".into()]).is_none());
        assert!(offscreen_argument(&["--new-window".into()])
            .is_some_and(|arg| arg.starts_with("--window-position=")));
    }
}

impl Profile {
    /// `--no-startup-window` keeps Chromium alive without windows. Once the
    /// task browser has shown a window and then has none for two checks, close
    /// it through its own proven endpoint. Never deletes the profile.
    pub(super) fn watch_lifetime(self: &Arc<Self>, pid: u32) {
        if self
            .watching
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let profile = self.clone();
        tokio::spawn(async move {
            let mut empty = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let Ok(record) = profile.verify() else { break };
                let Some(process) = record.process.clone() else {
                    break;
                };
                let alive = crate::browser_platform::process_identity(pid)
                    .is_ok_and(|(start, _)| process.start_time == Some(start));
                if !alive {
                    break;
                }
                let windows = crate::win32::list_windows_via_win32(Some(pid))
                    .into_iter()
                    .filter(|window| window.pid == pid)
                    .count();
                empty = if windows == 0 { empty + 1 } else { 0 };
                if empty < 2 {
                    continue;
                }
                if let Ok(Some(endpoint)) = endpoint_at_use(pid, &record.lifecycle).await {
                    if let Ok(conn) =
                        cua_driver_core::browser::cdp_ws::CdpConnection::connect(&endpoint.ws_url)
                            .await
                    {
                        let _ = conn.call(None, "Browser.close", json!({})).await;
                    }
                }
                break;
            }
            profile
                .watching
                .store(false, std::sync::atomic::Ordering::Release);
        });
    }
}

/// Browser frames only: owned bubbles and tool windows stay where the
/// browser anchors them.
fn is_browser_frame(hwnd: u64) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindow, GetWindowLongW, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
    };
    let hwnd = HWND(hwnd as *mut _);
    let owned = unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|owner| !owner.is_invalid());
    let tool = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW.0 != 0;
    !owned && !tool
}
