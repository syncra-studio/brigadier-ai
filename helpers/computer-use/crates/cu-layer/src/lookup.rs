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

/// The real lookups on this Mac.
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

#[cfg(not(target_os = "macos"))]
impl Lookups for System {
    fn app_for_pid(&self, _pid: i64) -> Option<TargetIdentity> {
        None
    }
    fn pid_for_window(&self, _window_id: u64) -> Option<i64> {
        None
    }
    fn identify(&self, _reference: &str) -> Option<(Option<String>, String)> {
        None
    }
    fn name_for_bundle(&self, _bundle_id: &str) -> Option<String> {
        None
    }
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
