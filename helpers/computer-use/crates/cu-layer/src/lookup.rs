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
        unsafe {
            let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid32)?;
            let bundle_id = app.bundleIdentifier().map(|value| value.to_string());
            let name = app
                .localizedName()
                .map(|value| value.to_string())
                .or_else(|| bundle_id.clone())
                .unwrap_or_default();
            Some(TargetIdentity {
                name,
                bundle_id,
                pid: Some(pid),
            })
        }
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
