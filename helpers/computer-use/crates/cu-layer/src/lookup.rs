//! The platform facts the guard needs about a target. Side-effect free and
//! needing no Accessibility or Screen Recording grant.

use crate::target_policy::TargetIdentity;

/// Lookups the guard makes; tests supply fakes.
pub trait Lookups: Send + Sync {
    /// The running app that owns `pid`, if it is a registered app.
    fn app_for_pid(&self, pid: i64) -> Option<TargetIdentity>;
    /// The process that owns a window-server window.
    fn pid_for_window(&self, window_id: u64) -> Option<i64>;
    /// An installed app named by display name or `.app` path: its bundle id
    /// when known and its name as its bundle is named.
    fn identify(&self, reference: &str) -> Option<(Option<String>, String)>;
    /// The name of the installed app with `bundle_id`.
    fn name_for_bundle(&self, bundle_id: &str) -> Option<String>;
}

/// The real lookups on this computer.
pub struct System;

#[cfg(target_os = "macos")]
impl Lookups for System {
    fn app_for_pid(&self, pid: i64) -> Option<TargetIdentity> {
        use objc2_app_kit::NSRunningApplication;
        let pid32 = i32::try_from(pid).ok().filter(|pid| *pid > 0)?;
        let running = unsafe {
            NSRunningApplication::runningApplicationWithProcessIdentifier(pid32).map(|app| {
                // A process AppKit knows no bundle of is named by the bundle its executable
                // runs from, if any: one without is a bare binary (`tauri dev`), which the
                // policy tells from the installed app by that (#510).
                let bundle_id = app
                    .bundleIdentifier()
                    .map(|value| value.to_string())
                    .or_else(|| bundle_id_at(&app_bundle_of(&executable_of(pid32)?)?));
                let name = app
                    .localizedName()
                    .map(|value| value.to_string())
                    .or_else(|| bundle_id.clone())
                    .unwrap_or_default();
                TargetIdentity {
                    name,
                    bundle_id,
                    pid: Some(pid),
                }
            })
        };
        // Just after the daemon starts, AppKit can know nothing yet of an app that is running,
        // and a target nobody can name passes the guard: the bundle its executable runs from
        // names it then.
        running.or_else(|| {
            let app = app_bundle_of(&executable_of(pid32)?)?;
            Some(TargetIdentity {
                name: stem(&app)?,
                bundle_id: bundle_id_at(&app),
                pid: Some(pid),
            })
        })
    }

    fn pid_for_window(&self, window_id: u64) -> Option<i64> {
        let window_id = u32::try_from(window_id).ok()?;
        platform_macos::windows::window_info_by_id(window_id).map(|window| i64::from(window.pid))
    }

    fn identify(&self, reference: &str) -> Option<(Option<String>, String)> {
        let path = std::path::Path::new(reference);
        if reference.ends_with(".app") && path.is_dir() {
            return Some((bundle_id_at(path), stem(path)?));
        }
        let name = reference.trim().trim_end_matches(".app");
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let roots = [
            Some(std::path::PathBuf::from("/Applications")),
            Some(std::path::PathBuf::from("/Applications/Utilities")),
            Some(std::path::PathBuf::from("/System/Applications")),
            Some(std::path::PathBuf::from("/System/Applications/Utilities")),
            home.map(|home| home.join("Applications")),
        ];
        roots
            .into_iter()
            .flatten()
            .map(|root| root.join(format!("{name}.app")))
            .find(|candidate| candidate.is_dir())
            .and_then(|found| Some((bundle_id_at(&found), stem(&found)?)))
    }

    fn name_for_bundle(&self, bundle_id: &str) -> Option<String> {
        use objc2_app_kit::NSWorkspace;
        use objc2_foundation::NSString;
        let path = unsafe {
            NSWorkspace::sharedWorkspace()
                .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?
                .path()?
                .to_string()
        };
        stem(std::path::Path::new(&path))
    }
}

/// Windows and Linux have no bundle ids. An app is named by its executable:
/// the id is the executable's lower-cased name without `.exe`
/// (`windowsterminal`, `gnome-terminal-server`), which the target policy and
/// the allow list compare. A program run from a Cargo build folder
/// (`target/debug`, `target/release`) is a development build and has no id,
/// like a bare binary on macOS (#510).
#[cfg(not(target_os = "macos"))]
impl Lookups for System {
    fn app_for_pid(&self, pid: i64) -> Option<TargetIdentity> {
        let pid = u32::try_from(pid).ok().filter(|pid| *pid > 0)?;
        let executable = executable_of(pid)?;
        let name = executable_name(&executable)?;
        Some(TargetIdentity {
            bundle_id: (!is_development_build(&executable)).then(|| name.to_ascii_lowercase()),
            name,
            pid: Some(i64::from(pid)),
        })
    }

    fn pid_for_window(&self, window_id: u64) -> Option<i64> {
        window_owner(window_id).map(i64::from)
    }

    fn identify(&self, reference: &str) -> Option<(Option<String>, String)> {
        let path = std::path::Path::new(reference.trim());
        let name = executable_name(path)?;
        // A path names the executable; a bare name is only a display name.
        let is_path = reference.contains('/') || reference.contains('\\');
        Some((is_path.then(|| name.to_ascii_lowercase()), name))
    }

    fn name_for_bundle(&self, _bundle_id: &str) -> Option<String> {
        None
    }
}

/// The executable's name without its directory or `.exe`.
#[cfg(not(target_os = "macos"))]
fn executable_name(path: &std::path::Path) -> Option<String> {
    let file = path.file_name()?.to_string_lossy().into_owned();
    let stem = match file.len().checked_sub(4) {
        Some(cut) if file[cut..].eq_ignore_ascii_case(".exe") => file[..cut].to_owned(),
        _ => file,
    };
    (!stem.is_empty()).then_some(stem)
}

/// Whether `executable` runs from a Cargo build folder.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn is_development_build(executable: &std::path::Path) -> bool {
    let parts: Vec<String> = executable
        .components()
        .map(|part| part.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    parts
        .windows(2)
        .any(|pair| pair[0] == "target" && matches!(pair[1].as_str(), "debug" | "release"))
        || parts.windows(3).any(|triple| {
            triple[0] == "target" && matches!(triple[2].as_str(), "debug" | "release")
        })
}

/// The executable `pid` runs.
#[cfg(target_os = "linux")]
fn executable_of(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(target_os = "windows")]
fn executable_of(pid: u32) -> Option<std::path::PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the handle is checked and closed; the buffer outlives the call
    // and its length is passed in and read back.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = vec![0u16; 32_768];
        let mut length = u32::try_from(buffer.len()).ok()?;
        let read = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        read.ok()?;
        buffer.truncate(usize::try_from(length).ok()?);
        Some(std::path::PathBuf::from(String::from_utf16(&buffer).ok()?))
    }
}

/// The process that owns a window: an X11 window's `_NET_WM_PID` on Linux.
#[cfg(target_os = "linux")]
fn window_owner(window_id: u64) -> Option<u32> {
    platform_linux::x11::window_pid(window_id)
}

/// The process that owns a top-level window (`HWND`).
#[cfg(target_os = "windows")]
fn window_owner(window_id: u64) -> Option<u32> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut pid = 0u32;
    // SAFETY: a stale or foreign handle makes the call return 0, which is
    // checked; `pid` is a valid out pointer.
    let thread =
        unsafe { GetWindowThreadProcessId(HWND(window_id as usize as *mut _), Some(&mut pid)) };
    (thread != 0 && pid != 0).then_some(pid)
}

/// The executable `pid` runs, read from the kernel.
#[cfg(target_os = "macos")]
fn executable_of(pid: i32) -> Option<std::path::PathBuf> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buffer` is writable for the length passed; the result is checked.
    let written = unsafe {
        libc::proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast(),
            u32::try_from(buffer.len()).ok()?,
        )
    };
    buffer.truncate(usize::try_from(written).ok().filter(|n| *n > 0)?);
    Some(std::path::PathBuf::from(String::from_utf8(buffer).ok()?))
}

/// The `.app` bundle whose main executable is `executable`
/// (`<bundle>.app/Contents/MacOS/<name>`).
#[cfg(target_os = "macos")]
fn app_bundle_of(executable: &std::path::Path) -> Option<std::path::PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && app.extension()? == "app")
        .then(|| app.to_path_buf())
}

#[cfg(target_os = "macos")]
fn stem(path: &std::path::Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

/// `CFBundleIdentifier` of the `.app` bundle at `path`.
#[cfg(target_os = "macos")]
pub(crate) fn bundle_id_at(path: &std::path::Path) -> Option<String> {
    use objc2_foundation::{NSBundle, NSString};
    let bundle = unsafe { NSBundle::bundleWithPath(&NSString::from_str(path.to_str()?)) }?;
    let bundle_id = unsafe { bundle.bundleIdentifier() }?.to_string();
    (!bundle_id.is_empty()).then_some(bundle_id)
}

/// The app a call acts on: by pid, by window, by bundle id or, for
/// `launch_app`, by name. One identity per app however a call names it, so
/// a stop by one name holds against the others. Listing tools name none.
pub fn target_app(
    lookups: &dyn Lookups,
    tool: &str,
    args: &serde_json::Value,
) -> Option<TargetIdentity> {
    use serde_json::Value;
    if matches!(tool, "list_windows" | "list_apps") {
        return None;
    }
    if let Some(pid) = args.get("pid").and_then(Value::as_i64) {
        return lookups.app_for_pid(pid);
    }
    if let Some(window_id) = args.get("window_id").and_then(Value::as_u64) {
        return lookups
            .pid_for_window(window_id)
            .and_then(|pid| lookups.app_for_pid(pid));
    }
    if let Some(bundle_id) = args.get("bundle_id").and_then(Value::as_str) {
        return Some(TargetIdentity {
            name: lookups
                .name_for_bundle(bundle_id)
                .unwrap_or_else(|| bundle_id.to_owned()),
            bundle_id: Some(bundle_id.to_owned()),
            pid: None,
        });
    }
    if tool == "launch_app" {
        if let Some(name) = args.get("name").and_then(Value::as_str) {
            if let Some((bundle_id, name)) = lookups.identify(name) {
                return Some(TargetIdentity {
                    name,
                    bundle_id,
                    pid: None,
                });
            }
            let name = name.trim_end_matches('/').trim_end_matches(".app");
            let name = name.rsplit('/').next().unwrap_or(name);
            return Some(TargetIdentity {
                name: name.to_owned(),
                bundle_id: None,
                pid: None,
            });
        }
    }
    None
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn an_app_is_named_by_the_bundle_its_executable_runs_from() {
        assert_eq!(
            app_bundle_of(Path::new("/Applications/Notes.app/Contents/MacOS/Notes")).as_deref(),
            Some(Path::new("/Applications/Notes.app"))
        );
        // A helper's own bundle, not the app around it.
        assert_eq!(
            app_bundle_of(Path::new(
                "/d/Brigadier.app/Contents/Helpers/Computer Use.app/Contents/MacOS/brigadier-cu"
            ))
            .as_deref(),
            Some(Path::new(
                "/d/Brigadier.app/Contents/Helpers/Computer Use.app"
            ))
        );
        assert_eq!(app_bundle_of(Path::new("/usr/bin/true")), None);
        assert_eq!(
            app_bundle_of(Path::new("/d/Tool.app/Contents/Resources/tool")),
            None
        );

        // What the fallback reads for a running app: Finder always runs.
        let out = std::process::Command::new("pgrep")
            .args(["-x", "Finder"])
            .output()
            .unwrap();
        let finder: i32 = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .next()
            .expect("Finder runs")
            .parse()
            .unwrap();
        let app = app_bundle_of(&executable_of(finder).unwrap()).unwrap();
        assert_eq!(stem(&app).as_deref(), Some("Finder"));
        assert_eq!(bundle_id_at(&app).as_deref(), Some("com.apple.finder"));
        assert_eq!(executable_of(-1), None);
    }
}

#[cfg(test)]
mod build_folder_tests {
    use super::is_development_build;
    use std::path::Path;

    #[test]
    fn a_program_in_a_cargo_build_folder_is_a_development_build() {
        assert!(is_development_build(Path::new(
            "/home/me/brigadier-ai/target/debug/brigadier-app"
        )));
        assert!(is_development_build(Path::new(
            "/home/me/brigadier-ai/target/x86_64-unknown-linux-gnu/release/brigadier-app"
        )));
        assert!(!is_development_build(Path::new("/usr/bin/brigadier")));
        assert!(!is_development_build(Path::new(
            "/opt/target/Brigadier/brigadier"
        )));
    }
}
