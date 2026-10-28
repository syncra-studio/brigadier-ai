//! Target guard: which apps and windows agents may act on.
//!
//! Every targeted call (a `pid`, a `window_id`, an element token, a
//! `bundle_id`, or the app `launch_app` opens) passes this policy in the
//! guard ([`crate::guard`]), which runs at the driver registry's dispatch
//! boundary, so a single call and each `run_steps` step pass the same check.
//! By default it refuses:
//!
//! - the user's Brigadier: `ai.brigadier.app` and every `ai.brigadier.app.*`
//!   bundle and their windows (development builds under other ids, and a
//!   bare `tauri dev` binary, stay targetable, so an agent can test the build
//!   it is working on);
//! - other terminal apps (Terminal, Ghostty, iTerm2, Warp, kitty, ...);
//! - the helper itself and the embedding host (`BRIGADIER_CU_HOST_BUNDLE_ID`);
//! - authentication and security surfaces (SecurityAgent, Keychain Access,
//!   Passwords, System Settings, ...; on Windows the credential and sign-in
//!   prompts; on Linux the polkit and keyring prompts). The system's settings
//!   app may be opened and read, never changed.
//!
//! Windows and Linux have no bundle ids: an app's id there is its
//! executable's lower-cased name without `.exe` ([`crate::lookup`]), and the
//! lists below name those too.
//!
//! A caller may unlock exact bundle ids with
//! [`ALLOWED_TARGET_BUNDLE_IDS_ENV`] on the process that serves the MCP
//! session. The list can unlock the host and other terminals. It never
//! unlocks the release Brigadier family, the helper, or the security surfaces.
//! The list reaches the guard through the trusted
//! [`crate::scope::DispatchScope`] of the call, never as a tool argument.

use cua_driver_core::protocol::ToolResult;
use serde_json::{json, Value};

/// Comma- or whitespace-separated exact bundle ids this MCP session may
/// target even though the default policy refuses them.
pub const ALLOWED_TARGET_BUNDLE_IDS_ENV: &str = "BRIGADIER_CU_ALLOWED_TARGET_BUNDLE_IDS";

/// The identity of a target app, as resolved by the platform.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetIdentity {
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: Option<i64>,
}

/// Platform lookups the policy needs. Both are side-effect free.
pub struct TargetResolver<'a> {
    /// The app that owns a process id, if it is a running app.
    pub app_for_pid: &'a dyn Fn(i64) -> Option<TargetIdentity>,
    /// The process id that owns a window id.
    pub pid_for_window: &'a dyn Fn(u64) -> Option<i64>,
}

/// Why a target was refused. The `as_str` value is the
/// `structuredContent.reason` of the refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefusalReason {
    /// The user's Brigadier (any `ai.brigadier.app*` bundle not unlocked).
    UserBrigadier,
    /// Another terminal app.
    Terminal,
    /// The driver, its helper, or the embedding host.
    Driver,
    /// An authentication or security surface of the operating system.
    SecuritySurface,
    /// A Windows Store app hosted by a shared application frame.
    StoreApp,
}

impl RefusalReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserBrigadier => "user_brigadier",
            Self::Terminal => "terminal",
            Self::Driver => "driver",
            Self::SecuritySurface => "security_surface",
            Self::StoreApp => "store_app_background_unsupported",
        }
    }
}

/// Parse the allow list from its env form: exact ids, lower-cased. Entries
/// with a wildcard are dropped, since the list only names exact bundles.
pub fn parse_allowed(raw: &str) -> Vec<String> {
    let mut out: Vec<String> = raw
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(|entry| entry.trim().to_ascii_lowercase())
        .filter(|entry| !entry.is_empty() && !entry.contains('*'))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The allow list of this process, from [`ALLOWED_TARGET_BUNDLE_IDS_ENV`].
pub fn allowed_from_env() -> Vec<String> {
    std::env::var(ALLOWED_TARGET_BUNDLE_IDS_ENV)
        .map(|raw| parse_allowed(&raw))
        .unwrap_or_default()
}

/// Tools that take a `pid` or `window_id` only to filter a listing. They read
/// no window content and act on nothing.
fn is_listing_tool(tool: &str) -> bool {
    matches!(tool, "list_windows" | "list_apps")
}

/// Check a native tool call against the policy. `Ok` for calls that name no
/// target or name an allowed one.
pub fn enforce(
    tool: &str,
    args: &Value,
    allowed: &[String],
    resolver: &TargetResolver<'_>,
) -> Result<(), ToolResult> {
    if is_listing_tool(tool) {
        return Ok(());
    }
    enforce_executable_launch(tool, args, allowed)?;
    let own_pid = i64::from(std::process::id());
    let mut checked_pid = None;
    if let Some(pid) = args.get("pid").and_then(Value::as_i64) {
        checked_pid = Some(pid);
        check_pid(tool, args, pid, own_pid, allowed, resolver)?;
    }
    if let Some(window_id) = args.get("window_id").and_then(Value::as_u64) {
        if let Some(owner) = (resolver.pid_for_window)(window_id) {
            if checked_pid != Some(owner) {
                check_pid(tool, args, owner, own_pid, allowed, resolver)?;
            }
        } else {
            #[cfg(not(target_os = "macos"))]
            return Err(unresolved_identity(tool));
        }
    }
    // `launch_app` and `page` may name their app by bundle id alone.
    if let Some(bundle_id) = args.get("bundle_id").and_then(Value::as_str) {
        let identity = TargetIdentity {
            name: bundle_id.to_owned(),
            bundle_id: Some(bundle_id.to_owned()),
            pid: None,
        };
        check_call(tool, args, &identity, allowed)?;
    }
    if tool == "launch_app" {
        if let Some(name) = args.get("name").and_then(Value::as_str) {
            let identity = TargetIdentity {
                name: app_name_from_reference(name),
                bundle_id: None,
                pid: None,
            };
            check_call(tool, args, &identity, allowed)?;
        }
        if let Some(urls) = args.get("urls").and_then(Value::as_array) {
            for url in urls.iter().filter_map(Value::as_str) {
                if url_opens_brigadier(url) {
                    let identity = TargetIdentity {
                        name: url.to_owned(),
                        bundle_id: None,
                        pid: None,
                    };
                    return Err(refusal(tool, &identity, RefusalReason::UserBrigadier));
                }
            }
        }
    }
    Ok(())
}

fn check_pid(
    tool: &str,
    args: &Value,
    pid: i64,
    own_pid: i64,
    allowed: &[String],
    resolver: &TargetResolver<'_>,
) -> Result<(), ToolResult> {
    let identity = (resolver.app_for_pid)(pid);
    if pid == own_pid {
        let identity = identity.unwrap_or(TargetIdentity {
            name: "brigadier-cu".to_owned(),
            bundle_id: None,
            pid: Some(pid),
        });
        return Err(refusal(tool, &identity, RefusalReason::Driver));
    }
    match identity {
        Some(identity) => {
            check_call(tool, args, &identity, allowed)?;
            #[cfg(not(target_os = "macos"))]
            {
                let unlocked = identity
                    .bundle_id
                    .as_ref()
                    .is_some_and(|id| allowed.iter().any(|entry| entry.eq_ignore_ascii_case(id)));
                if !unlocked && process_hosts_terminal(pid) {
                    return Err(refusal(tool, &identity, RefusalReason::Terminal));
                }
            }
            Ok(())
        }
        None => {
            #[cfg(target_os = "macos")]
            {
                Ok(())
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(unresolved_identity(tool))
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn unresolved_identity(tool: &str) -> ToolResult {
    ToolResult::error(format!("target_identity_unavailable: {tool} could not identify the target process/window; refresh list_windows and use an identifiable app. No action was attempted."))
        .with_structured(json!({"code":"target_identity_unavailable"}))
}

/// Check one resolved identity.
pub fn check(tool: &str, identity: &TargetIdentity, allowed: &[String]) -> Result<(), ToolResult> {
    match classify(identity, allowed) {
        Some(RefusalReason::SecuritySurface)
            if reads_settings(tool, identity)
                || tool == "kill_app" && is_windows_settings(identity) =>
        {
            Ok(())
        }
        Some(reason) => Err(refusal(tool, identity, reason)),
        None => Ok(()),
    }
}

fn check_call(
    tool: &str,
    args: &Value,
    identity: &TargetIdentity,
    allowed: &[String],
) -> Result<(), ToolResult> {
    if is_windows_settings(identity)
        && classify(identity, allowed) == Some(RefusalReason::SecuritySurface)
        && cua_driver_core::window_state_view::windows_settings_close_request(tool, args)
    {
        return Ok(());
    }
    check(tool, identity, allowed)
}

/// System Settings may be opened and read (a version, whether a setting is
/// on), never changed: an agent must not grant itself permissions there.
fn reads_settings(tool: &str, identity: &TargetIdentity) -> bool {
    is_settings(identity)
        && matches!(
            tool,
            "launch_app" | "get_window_state" | "verify_state" | "get_accessibility_tree" | "zoom"
        )
}

/// Windows Settings owns a separate app process; terminating it cannot change
/// a setting. Other security surfaces and platform Settings retain their policy.
fn is_windows_settings(identity: &TargetIdentity) -> bool {
    let id = identity
        .bundle_id
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    id == "systemsettings" || id.starts_with("windows.immersivecontrolpanel")
}

/// Whether `identity` is System Settings.
fn is_settings(identity: &TargetIdentity) -> bool {
    let bundle = identity.bundle_id.as_deref().unwrap_or_default();
    let name = identity.name.trim().to_ascii_lowercase();
    bundle.eq_ignore_ascii_case("com.apple.systempreferences")
        // Windows Settings, KDE System Settings and GNOME Settings.
        || matches!(
            bundle.to_ascii_lowercase().as_str(),
            "systemsettings" | "systemsettings5" | "systemsettings6" | "gnome-control-center"
                | "xfce4-settings-manager" | "xfce4-settings-editor" | "mate-control-center" | "cinnamon-settings"
        )
        || bundle
            .to_ascii_lowercase()
            .starts_with("windows.immersivecontrolpanel")
        || (bundle.is_empty()
            && matches!(
                name.as_str(),
                "system settings" | "system preferences" | "settings"
            ))
}

/// The refusal reason for `identity`, or `None` when it may be targeted.
pub fn classify(identity: &TargetIdentity, allowed: &[String]) -> Option<RefusalReason> {
    let bundle = identity
        .bundle_id
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = identity.name.trim().to_ascii_lowercase();
    let unlocked = !bundle.is_empty() && allowed.contains(&bundle);

    if is_driver(&bundle, &name) {
        return Some(RefusalReason::Driver);
    }
    if bundle == "applicationframehost" || name == "applicationframehost" {
        return Some(RefusalReason::StoreApp);
    }
    if is_security_surface(&bundle, &name) {
        return Some(RefusalReason::SecuritySurface);
    }
    if is_brigadier(&bundle, &name, identity.pid) {
        return Some(RefusalReason::UserBrigadier);
    }
    let host = std::env::var(crate::HOST_BUNDLE_ID_ENV)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !host.is_empty() && bundle == host && !unlocked {
        return Some(RefusalReason::Driver);
    }
    if is_terminal(&bundle, &name) && !unlocked {
        return Some(RefusalReason::Terminal);
    }
    None
}

fn is_driver(bundle: &str, name: &str) -> bool {
    matches!(
        bundle,
        "ai.brigadier.computer-use" | "ai.brigadier.dev.computer-use"
    ) || bundle.starts_with("ai.brigadier.computer-use.")
        || bundle == "brigadier-cu"
        || matches!(name, "brigadier-cu" | "brigadier computer use")
}

fn is_brigadier(bundle: &str, name: &str, pid: Option<i64>) -> bool {
    // The name only decides when the bundle is unknown (launch by name), so
    // a development build named "Brigadier Dev" is not mistaken for it. A
    // macOS process without a bundle is a bare binary (`tauri dev`), never
    // the installed app, which runs from its bundle (#510). Windows dev
    // builds have the same executable id as the installed app: use its path.
    bundle == "ai.brigadier.app"
        || bundle.starts_with("ai.brigadier.app.")
        || (matches!(bundle, "brigadier" | "brigadier-app" | "brigadierd")
            && is_installed_brigadier_process(pid))
        || (bundle.is_empty() && pid.is_none() && name == "brigadier")
}

#[cfg(not(target_os = "windows"))]
fn is_installed_brigadier_process(_pid: Option<i64>) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn is_installed_brigadier_process(pid: Option<i64>) -> bool {
    let executable = pid
        .and_then(|pid| u32::try_from(pid).ok())
        .and_then(crate::lookup::executable_of);
    let local_app_data = std::env::var("LOCALAPPDATA").ok();
    windows_brigadier_path_is_protected(
        executable.as_deref().and_then(std::path::Path::to_str),
        local_app_data.as_deref(),
    )
}

/// NSIS currentUser + productName Brigadier installs under LOCALAPPDATA/Brigadier.
/// Unknown paths stay protected. Compare directory boundaries, ignoring Windows
/// casing, slash style and the extended-length prefix returned by some APIs.
#[cfg(any(target_os = "windows", test))]
fn windows_brigadier_path_is_protected(
    executable: Option<&str>,
    local_app_data: Option<&str>,
) -> bool {
    fn normalize(path: &str) -> Option<String> {
        let path = path.replace('/', "\\").to_lowercase();
        let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
        if !(path.as_bytes().get(1) == Some(&b':') && path.as_bytes().get(2) == Some(&b'\\'))
            && !path.starts_with(r"\\")
        {
            return None;
        }
        let mut parts = Vec::new();
        for part in path.split('\\') {
            match part {
                "." => {}
                ".." => {
                    parts.pop()?;
                }
                _ => parts.push(part),
            }
        }
        Some(parts.join("\\").trim_end_matches('\\').to_owned())
    }
    let Some(executable) = executable.and_then(normalize) else {
        return true;
    };
    let Some(local_app_data) = local_app_data.and_then(normalize) else {
        return true;
    };
    executable.starts_with(&format!("{local_app_data}\\brigadier\\"))
}

fn is_security_surface(bundle: &str, name: &str) -> bool {
    matches!(
        bundle,
        "com.apple.loginwindow"
            | "com.apple.securityagent"
            | "com.apple.coreservicesuiagent"
            | "com.apple.authorizationhost"
            | "com.apple.keychainaccess"
            | "com.apple.passwords"
            | "com.apple.systempreferences"
    ) || bundle.starts_with("com.apple.localauthentication.")
        || bundle.starts_with("com.apple.authenticationservices.")
        || is_windows_or_linux_security_surface(bundle)
        || matches!(
            name,
            "loginwindow"
                | "securityagent"
                | "authorizationhost"
                | "coreauthd"
                | "localauthenticationremoteservice"
                | "keychain access"
                | "passwords"
                | "system settings"
                | "system preferences"
        )
}

/// Windows sign-in, credential and elevation prompts; Linux polkit agents,
/// keyring and PIN prompts and password managers that ship with the desktop.
/// Settings apps are here too, so they can be read but not changed.
fn is_windows_or_linux_security_surface(bundle: &str) -> bool {
    matches!(
        bundle,
        "consent"
            | "credentialuibroker"
            | "logonui"
            | "lockapp"
            | "userpackagemanager"
            | "systemsettings"
            | "gnome-control-center"
            | "systemsettings5"
            | "systemsettings6"
            | "xfce4-settings-manager"
            | "xfce4-settings-editor"
            | "mate-control-center"
            | "cinnamon-settings"
            | "control"
            | "regedit"
            | "mmc"
            | "rundll32"
            | "gnome-shell"
            | "lxpolkit"
            | "gcr-prompter"
            | "gcr-viewer"
            | "seahorse"
            | "kwalletd5"
            | "kwalletd6"
            | "kwalletmanager5"
            | "ksecretd"
    ) || bundle.starts_with("windows.immersivecontrolpanel")
        || bundle.starts_with("polkit-")
        || bundle.starts_with("pinentry")
        || bundle.ends_with("-authentication-agent-1")
}

/// Terminal apps other than Brigadier.
pub fn is_terminal(bundle: &str, name: &str) -> bool {
    matches!(
        bundle,
        "com.apple.terminal"
            | "com.mitchellh.ghostty"
            | "com.googlecode.iterm2"
            | "net.kovidgoyal.kitty"
            | "org.alacritty"
            | "com.github.wez.wezterm"
            | "co.zeit.hyper"
            | "co.vercel.hyper"
    ) || (cfg!(not(target_os = "macos"))
        && matches!(
            bundle,
            "org.tabby"
                | "com.electron.tabby"
                | "com.waveterm.waveterm"
                | "com.github.railsware.upterm"
        ))
        || bundle.starts_with("dev.warp.")
        || is_windows_or_linux_terminal(bundle)
        || is_windows_or_linux_terminal(name)
        || name == "wmux"
        || matches!(
            name,
            "windows terminal"
                | "command prompt"
                | "git bash"
                | "git cmd"
                | "windows command processor"
                | "windows console host"
        )
        || name == "windows powershell"
        || name.starts_with("windows powershell ")
        || name.starts_with("powershell ")
        || matches!(
            name,
            "terminal"
                | "ghostty"
                | "iterm"
                | "iterm2"
                | "warp"
                | "warp preview"
                | "kitty"
                | "alacritty"
                | "wezterm"
                | "hyper"
        )
        || (cfg!(not(target_os = "macos"))
            && matches!(
                name,
                "wmux"
                    | "tabby"
                    | "git bash"
                    | "wezterm-gui"
                    | "windows terminal"
                    | "command prompt"
                    | "powershell"
                    | "konsole"
                    | "xterm"
                    | "gnome terminal"
                    | "console"
                    | "ptyxis"
            ))
}

/// Terminals and shells on Windows (executables and the Windows Terminal
/// package id) and Linux.
fn is_windows_or_linux_terminal(bundle: &str) -> bool {
    (cfg!(not(target_os = "macos"))
        && matches!(
            bundle,
            "wmux"
                | "tabby"
                | "hyper"
                | "wezterm"
                | "warp-preview"
                | "warp-terminal"
                | "git-bash"
                | "git-cmd"
        ))
        || matches!(
            bundle,
            "windowsterminal"
                | "wt"
                | "openconsole"
                | "conhost"
                | "cmd"
                | "powershell"
                | "powershell_ise"
                | "pwsh"
                | "mintty"
                | "wezterm-gui"
                | "alacritty"
                | "kitty"
                | "gnome-terminal"
                | "gnome-terminal-server"
                | "kgx"
                | "ptyxis"
                | "ptyxis-agent"
                | "konsole"
                | "xterm"
                | "uxterm"
                | "urxvt"
                | "rxvt"
                | "st"
                | "xfce4-terminal"
                | "mate-terminal"
                | "lxterminal"
                | "qterminal"
                | "terminator"
                | "tilix"
                | "foot"
                | "footclient"
                | "ghostty"
                | "warp"
                | "terminology"
                | "cool-retro-term"
                | "blackbox"
                | "bash"
                | "sh"
                | "dash"
                | "zsh"
                | "fish"
                | "ksh"
                | "wsl"
                | "wslhost"
                | "env"
        )
        || bundle.starts_with("microsoft.windowsterminal")
}

// Process evidence catches renamed/unlisted terminal hosts as well as the
// maintained app identities above. Never attach to a console or inject input.
#[cfg(any(not(target_os = "macos"), test))]
#[derive(Debug)]
struct ProcessFact {
    pid: i64,
    parent: i64,
    executable: String,
    has_tty: bool,
}

#[cfg(any(not(target_os = "macos"), test))]
fn hosts_terminal(pid: i64, processes: &[ProcessFact], windows: bool) -> bool {
    let parents: std::collections::HashMap<i64, i64> = processes
        .iter()
        .map(|process| (process.pid, process.parent))
        .collect();
    processes.iter().any(|process| {
        let executable = process.executable.to_ascii_lowercase();
        let executable = executable.rsplit(['/', '\\']).next().unwrap_or(&executable);
        let executable = executable.strip_suffix(".exe").unwrap_or(executable);
        // conhost/OpenConsole include headless ConPTY servers. Unix shell
        // children count only with a controlling PTY, not background scripts.
        let evidence = if windows {
            matches!(executable, "conhost" | "openconsole")
                || is_windows_or_linux_terminal(executable)
        } else {
            process.has_tty && is_windows_or_linux_terminal(executable)
        };
        if !evidence {
            return false;
        }
        let mut ancestor = process.pid;
        // Bound traversal and tolerate exit races, cycles and reparenting.
        for _ in 0..128 {
            if ancestor == pid {
                return true;
            }
            let Some(parent) = parents.get(&ancestor).copied() else {
                return false;
            };
            if parent <= 0 || parent == ancestor {
                return false;
            }
            ancestor = parent;
        }
        false
    })
}

#[cfg(target_os = "linux")]
fn process_hosts_terminal(pid: i64) -> bool {
    let Ok(mut child) = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,ppid=,tty=,comm="])
        .stdout(std::process::Stdio::piped())
        .spawn()
    else {
        return false;
    };
    // Drain concurrently: a full stdout pipe must not turn the deadline into
    // a permanent refusal on machines with many processes.
    use std::io::Read;
    let Some(mut stdout) = child.stdout.take() else {
        return false;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        let _ = tx.send(bytes);
    });
    let output = rx.recv_timeout(std::time::Duration::from_millis(250));
    // Even after stdout closes, do not wait indefinitely for ps to exit.
    let _ = child.kill();
    let _ = child.wait();
    let Ok(output) = output else {
        return false;
    };
    let processes: Vec<ProcessFact> = String::from_utf8_lossy(&output)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let parent = fields.next()?.parse().ok()?;
            let tty = fields.next()?;
            Some(ProcessFact {
                pid,
                parent,
                executable: fields.collect::<Vec<_>>().join(" "),
                has_tty: !matches!(tty, "?" | "??" | "-"),
            })
        })
        .collect();
    hosts_terminal(pid, &processes, false)
}

#[cfg(windows)]
fn process_hosts_terminal(pid: i64) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: the snapshot is closed on every successful acquisition; each
    // PROCESSENTRY32W has the required size and a live, fixed-size name buffer.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut processes = Vec::new();
        let mut next = Process32FirstW(snapshot, &mut entry);
        while next.is_ok() {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            processes.push(ProcessFact {
                pid: i64::from(entry.th32ProcessID),
                parent: i64::from(entry.th32ParentProcessID),
                executable: String::from_utf16_lossy(&entry.szExeFile[..len]),
                has_tty: false,
            });
            next = Process32NextW(snapshot, &mut entry);
        }
        let _ = CloseHandle(snapshot);
        hosts_terminal(pid, &processes, true)
    }
}

/// Native launch aliases do not pass through the macOS `name`/`bundle_id`
/// path. Check the actual command token as well as the display name.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn enforce_executable_launch(
    tool: &str,
    args: &Value,
    allowed: &[String],
) -> Result<(), ToolResult> {
    if tool != "launch_app" {
        return Ok(());
    }
    for key in ["name", "bundle_id", "aumid", "path", "launch_path"] {
        let Some(reference) = args[key].as_str() else {
            continue;
        };
        let reference = reference.trim();
        check(
            tool,
            &TargetIdentity {
                name: app_name_from_reference(reference),
                bundle_id: None,
                pid: None,
            },
            allowed,
        )?;
        let token = if let Some(quoted) = reference.strip_prefix('"') {
            quoted.split('"').next().unwrap_or(quoted)
        } else if reference.contains('!') {
            reference
        } else if reference.to_ascii_lowercase().contains(".exe") {
            let end = reference.to_ascii_lowercase().find(".exe").unwrap() + 4;
            &reference[..end]
        } else {
            reference.split_whitespace().next().unwrap_or(reference)
        };
        let executable = token.rsplit(['/', '\\']).next().unwrap_or(token);
        let id = executable.to_ascii_lowercase();
        let id = id.strip_suffix(".exe").unwrap_or(&id);
        let id = id.strip_suffix(".desktop").unwrap_or(id);
        // XDG desktop launchers commonly use `env NAME=value app`. Check
        // the app rather than disabling those GUI launchers. More complex
        // env flags fail closed, including -S (which can hide a shell).
        let wrapped;
        let id = if id == "env" {
            let mut words = reference.split_whitespace().skip(1).chain(
                args["additional_arguments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            );
            let command = words.find(|word| !word.contains('='));
            match command.filter(|word| !word.starts_with('-')) {
                Some(command) => {
                    wrapped = command
                        .rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(command)
                        .to_ascii_lowercase();
                    wrapped.strip_suffix(".exe").unwrap_or(&wrapped)
                }
                None => id,
            }
        } else {
            id
        };
        check(
            tool,
            &TargetIdentity {
                name: id.to_owned(),
                bundle_id: Some(id.to_owned()),
                pid: None,
            },
            allowed,
        )?;
    }
    Ok(())
}

/// `launch_app` accepts a display name or an app path; compare the app name.
fn app_name_from_reference(reference: &str) -> String {
    let trimmed = reference
        .trim()
        .trim_matches('"')
        .trim_end_matches(['/', '\\']);
    let last = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    let lower = last.to_ascii_lowercase();
    for suffix in [".app", ".exe", ".lnk", ".desktop"] {
        if lower.ends_with(suffix) {
            return last[..last.len() - suffix.len()].to_owned();
        }
    }
    last.to_owned()
}

/// A `brigadier:` (or `brigadier-*:`) URL opens in the user's Brigadier.
fn url_opens_brigadier(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    let scheme = scheme.to_ascii_lowercase();
    scheme == "brigadier" || scheme.starts_with("brigadier-") || scheme.starts_with("brigadier+")
}

#[cfg(target_os = "macos")]
const SETTINGS_REFUSAL: &str =
    "System Settings can be opened and read, never clicked or typed into. To read a \
     pane, open it directly: launch_app with bundle_id com.apple.systempreferences and \
     urls [\"x-apple.systempreferences:<pane id>\"], for example \
     com.apple.SystemProfiler.AboutExtension (General › About, the macOS version), \
     com.apple.Software-Update-Settings.extension, com.apple.Displays-Settings.extension, \
     com.apple.Network-Settings.extension, com.apple.BluetoothSettings or \
     com.apple.settings.PrivacySecurity.extension, then get_window_state. To change a \
     setting, ask the user to do this step.";
#[cfg(target_os = "windows")]
const SETTINGS_REFUSAL: &str =
    "Settings can be opened, read and closed; all other input is refused. Close a task-owned window with click using pid, window_id and action:\"close\" without an element or pixels; use kill_app if normal close fails. To read a page, open it \
     directly: launch_app with urls [\"ms-settings:<page>\"], for example ms-settings:about \
     (the Windows version), ms-settings:windowsupdate, ms-settings:display or \
     ms-settings:network, then get_window_state. To change a setting, ask the user to do \
     this step.";
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
const SETTINGS_REFUSAL: &str =
    "The system settings can be opened and read, never clicked or typed into: open them \
     with launch_app, then get_window_state. To change a setting, ask the user to do this \
     step.";

#[cfg(target_os = "macos")]
const SECURITY_REFUSAL: &str =
    "Brigadier Computer Use does not act on macOS authentication or security surfaces \
     (System Settings can be opened and read, not changed). Ask the user to do this step.";
#[cfg(not(target_os = "macos"))]
const SECURITY_REFUSAL: &str =
    "Brigadier Computer Use does not act on the system's sign-in, password, credential or \
     elevation prompts (Settings can be opened and read, not changed). Ask the user to do \
     this step.";

/// The typed refusal returned to the model.
pub fn refusal(tool: &str, identity: &TargetIdentity, reason: RefusalReason) -> ToolResult {
    let bundle = identity.bundle_id.as_deref().unwrap_or("unknown bundle");
    let why = match reason {
        RefusalReason::UserBrigadier => {
            "Brigadier Computer Use never acts on the user's Brigadier app or its windows. Use the \
             Brigadier CLI, the Brigadier MCP tools, or the brigadier-browser skill to work with Brigadier; a \
             development build (Brigadier Dev, or one started with `tauri dev`) can be driven."
        }
        RefusalReason::Terminal => {
            "Brigadier Computer Use does not act on terminal apps by default. Run the command \
             in your own shell instead."
        }
        RefusalReason::Driver => {
            "Brigadier Computer Use does not act on itself or on the app that hosts it."
        }
        RefusalReason::SecuritySurface if is_settings(identity) => SETTINGS_REFUSAL,
        RefusalReason::SecuritySurface => SECURITY_REFUSAL,
        RefusalReason::StoreApp => "Store apps can't be driven in the background on Windows yet. Ask the user to do this step.",
    };
    let message = format!(
        "target_not_allowed: {tool} refused to target '{}' ({bundle}). {why}",
        identity.name
    );
    ToolResult::error(message.clone()).with_structured(json!({
        "code": "target_not_allowed",
        "reason": reason.as_str(),
        "tool": tool,
        "app": identity.name,
        "bundle_id": identity.bundle_id,
        "pid": identity.pid,
        "message": message,
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn windows_brigadier_dev_paths_are_allowed() {
        let local = Some(r"C:\Users\me\AppData\Local");
        for path in [
            r"C:\src\brigadier\target\debug\brigadier-app.exe",
            r"C:\src\brigadier\target\release\brigadier-app.exe",
            r"D:\test-app\brigadier-app.exe",
            r"C:\Users\me\AppData\Local\Brigadier-dev\brigadier-app.exe",
        ] {
            assert!(
                !super::windows_brigadier_path_is_protected(Some(path), local),
                "{path}"
            );
        }
    }

    #[test]
    fn windows_installed_brigadier_paths_are_refused() {
        let local = Some(r"C:\Users\me\AppData\Local\");
        for path in [
            r"C:\Users\me\AppData\Local\Brigadier\brigadier-app.exe",
            r"C:\Users\me\AppData\Local\Brigadier\brigadier.exe",
            r"C:\Users\me\AppData\Local\Brigadier\brigadierd.exe",
            r"c:/users/ME/appdata/local/BRIGADIER/brigadier-app.exe",
            r"\\?\C:\Users\me\AppData\Local\Brigadier\brigadier-app.exe",
            r"C:\Users\me\AppData\Local\Brigadier\target\debug\brigadier-app.exe",
            r"C:\Users\me\AppData\Local\other\..\Brigadier\brigadier-app.exe",
        ] {
            assert!(
                super::windows_brigadier_path_is_protected(Some(path), local),
                "{path}"
            );
        }
    }

    #[test]
    fn windows_unreadable_brigadier_paths_are_refused() {
        assert!(super::windows_brigadier_path_is_protected(
            None,
            Some(r"C:\Users\me\AppData\Local")
        ));
        assert!(super::windows_brigadier_path_is_protected(
            Some(r"C:\dev\brigadier-app.exe"),
            None
        ));
        assert!(super::windows_brigadier_path_is_protected(
            Some("brigadier-app.exe"),
            Some(r"C:\Users\me\AppData\Local")
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_brigadier_policy_uses_pid_path_and_fails_closed() {
        let mut identity = super::TargetIdentity {
            name: "brigadier-app".into(),
            bundle_id: Some("brigadier-app".into()),
            pid: Some(i64::from(std::process::id())),
        };
        // This test binary runs outside the installed app's folder.
        assert_eq!(super::classify(&identity, &[]), None);
        for pid in [None, Some(-1), Some(0)] {
            identity.pid = pid;
            assert_eq!(
                super::classify(&identity, &[]),
                Some(super::RefusalReason::UserBrigadier)
            );
        }
    }

    use super::*;

    fn process(pid: i64, parent: i64, executable: &str, has_tty: bool) -> ProcessFact {
        ProcessFact {
            pid,
            parent,
            executable: executable.into(),
            has_tty,
        }
    }

    #[test]
    fn terminal_display_names_paths_and_shell_aliases_are_refused_before_launch() {
        with_fake_apps(|resolver| {
            for name in [
                "Windows PowerShell",
                "Windows PowerShell (x86)",
                "Windows PowerShell ISE",
                "Git CMD",
                "Windows Command Processor",
                "Windows Console Host",
                "PowerShell 7 (x64)",
                "powershell",
                "pwsh",
                "cmd",
                "wt",
                "Windows Terminal",
                "wmux",
                "bash",
                "wsl",
                "conhost",
                "zsh",
                "fish",
                "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
                "C:\\Users\\user\\Windows PowerShell.lnk",
                "/bin/bash",
                "/Applications/Terminal.app",
            ] {
                for key in ["name", "path", "launch_path"] {
                    let args = json!({key:name});
                    assert_eq!(
                        reason_of(enforce("launch_app", &args, &[], resolver)).as_deref(),
                        Some("terminal"),
                        "{args}"
                    );
                }
            }
            for name in ["Notepad", "Calculator", "/Applications/Console.app"] {
                assert!(
                    enforce("launch_app", &json!({"name":name}), &[], resolver).is_ok(),
                    "{name}"
                );
            }
        });
    }

    #[test]
    fn console_and_pty_descendants_reveal_unknown_terminal_hosts() {
        for host in ["conhost.exe", "OpenConsole.exe", "pwsh.exe"] {
            let processes = [
                process(10, 1, "renamed-host.exe", false),
                process(11, 10, "broker.exe", false),
                process(12, 11, host, false),
            ];
            assert!(hosts_terminal(10, &processes, true), "{host}");
            assert!(!hosts_terminal(99, &processes, true));
        }
        let shell = [
            process(20, 1, "unknown-gui", false),
            process(21, 20, "/bin/zsh", true),
        ];
        assert!(hosts_terminal(20, &shell, false));
        let script = [
            process(20, 1, "ordinary-gui", false),
            process(21, 20, "/bin/zsh", false),
        ];
        assert!(!hosts_terminal(20, &script, false));
        assert!(!hosts_terminal(20, &[process(21, 21, "sh", true)], false));
    }

    #[test]
    fn maintained_terminal_names_cover_all_three_platforms() {
        for terminal in [
            "wmux",
            "wezterm",
            "wezterm-gui",
            "alacritty",
            "tabby",
            "hyper",
            "warp",
            "kitty",
            "mintty",
            "git-bash",
            "cmd",
            "pwsh",
            "ghostty",
            "gnome-terminal",
            "konsole",
            "xterm",
            "foot",
            "tilix",
        ] {
            if cfg!(target_os = "macos") && matches!(terminal, "tabby" | "git-bash") {
                assert!(!is_terminal(terminal, terminal), "{terminal}");
                continue;
            }
            assert!(is_terminal(terminal, terminal), "{terminal}");
            let target = TargetIdentity {
                name: terminal.into(),
                bundle_id: Some(terminal.into()),
                pid: None,
            };
            assert_eq!(classify(&target, &[]), Some(RefusalReason::Terminal));
            assert_eq!(classify(&target, &[terminal.into()]), None);
        }
        for (bundle, name) in [
            ("com.googlecode.iterm2", "iTerm2"),
            ("com.mitchellh.ghostty", "Ghostty"),
            ("com.github.wez.wezterm", "WezTerm"),
        ] {
            assert!(is_terminal(bundle, &name.to_ascii_lowercase()));
        }
        #[cfg(target_os = "macos")]
        assert!(!is_terminal("com.apple.console", "console"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_shell_launches_are_refused_before_native_target_lookup() {
        let resolver = TargetResolver {
            app_for_pid: &|_| panic!("shell launch must be refused before native lookup"),
            pid_for_window: &|_| panic!("shell launch must be refused before native lookup"),
        };
        for args in [
            json!({"name":"Windows PowerShell"}),
            json!({"path":"C:\\Windows\\System32\\cmd.exe"}),
            json!({"launch_path":"wt.exe"}),
        ] {
            assert_eq!(
                reason_of(enforce("launch_app", &args, &[], &resolver)).as_deref(),
                Some("terminal")
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_snapshot_does_not_attach_a_console() {
        // Only a read-only Toolhelp snapshot; the test harness has no terminal
        // descendants and looking up a nonexistent PID cannot find a host.
        assert!(!process_hosts_terminal(i64::from(u32::MAX)));
    }

    #[test]
    fn store_frame_refusal_names_background_support_not_credentials() {
        let identity = TargetIdentity {
            name: "ApplicationFrameHost".into(),
            bundle_id: Some("applicationframehost".into()),
            pid: Some(28908),
        };
        let result = check("get_window_state", &identity, &[]).unwrap_err();
        let state = result.structured_content.unwrap();
        assert_eq!(state["reason"], "store_app_background_unsupported");
        let message = state["message"].as_str().unwrap();
        assert!(message.contains("Store apps can't be driven in the background on Windows yet"));
        assert!(message.contains("Ask the user to do this step"));
        assert!(!message.contains("credential"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_console_is_not_a_windows_terminal() {
        let console = TargetIdentity {
            name: "Console".into(),
            bundle_id: Some("com.apple.Console".into()),
            pid: Some(42),
        };
        assert_eq!(classify(&console, &[]), None);
    }

    fn app(name: &str, bundle: &str, pid: i64) -> TargetIdentity {
        TargetIdentity {
            name: name.to_owned(),
            bundle_id: Some(bundle.to_owned()),
            pid: Some(pid),
        }
    }

    /// pid 100 = the user's Brigadier, 101 = a tagged Brigadier DEV build,
    /// 102 = TextEdit, 103 = Terminal, 104 = Brigadier NIGHTLY, 105 = Keychain
    /// Access, 106 = System Settings, 107 = a bare `brigadier` binary started
    /// with `tauri dev`. Window 9000 belongs to the user's Brigadier, 9001 to
    /// TextEdit, 9002 to System Settings.
    fn with_fake_apps<T>(body: impl FnOnce(&TargetResolver<'_>) -> T) -> T {
        let app_for_pid = |pid: i64| match pid {
            100 => Some(app("Brigadier", "ai.brigadier.app", 100)),
            101 => Some(app("Brigadier DEV agt1", "ai.brigadier.dev.agt1", 101)),
            102 => Some(app("TextEdit", "com.apple.TextEdit", 102)),
            103 => Some(app("Terminal", "com.apple.Terminal", 103)),
            104 => Some(app("Brigadier NIGHTLY", "ai.brigadier.app.nightly", 104)),
            105 => Some(app("Keychain Access", "com.apple.keychainaccess", 105)),
            106 => Some(app("System Settings", "com.apple.systempreferences", 106)),
            107 => Some(TargetIdentity {
                name: "brigadier".to_owned(),
                bundle_id: None,
                pid: Some(107),
            }),
            _ => None,
        };
        let pid_for_window = |window: u64| match window {
            9000 => Some(100),
            9001 => Some(102),
            9002 => Some(106),
            _ => None,
        };
        body(&TargetResolver {
            app_for_pid: &app_for_pid,
            pid_for_window: &pid_for_window,
        })
    }

    fn reason_of(result: Result<(), ToolResult>) -> Option<String> {
        let error = result.err()?;
        assert_eq!(error.is_error, Some(true));
        let structured = error.structured_content.expect("typed refusal");
        assert_eq!(structured["code"], "target_not_allowed");
        Some(structured["reason"].as_str().unwrap().to_owned())
    }

    #[test]
    fn a_call_on_the_users_brigadier_is_refused() {
        with_fake_apps(|resolver| {
            for tool in [
                "click",
                "type_text",
                "press_key",
                "get_window_state",
                "kill_app",
                "bring_to_front",
            ] {
                let refused = reason_of(enforce(
                    tool,
                    &json!({"pid": 100, "x": 1, "y": 1}),
                    &[],
                    resolver,
                ));
                assert_eq!(
                    refused.as_deref(),
                    Some("user_brigadier"),
                    "{tool} on ai.brigadier.app"
                );
            }
            let nightly = reason_of(enforce("click", &json!({"pid": 104}), &[], resolver));
            assert_eq!(nightly.as_deref(), Some("user_brigadier"));
        });
    }

    #[test]
    fn a_call_on_another_app_is_not_refused() {
        with_fake_apps(|resolver| {
            assert!(enforce("click", &json!({"pid": 102, "x": 1, "y": 1}), &[], resolver).is_ok());
            assert!(enforce(
                "get_window_state",
                &json!({"pid": 102, "window_id": 9001}),
                &[],
                resolver
            )
            .is_ok());
            assert!(enforce("get_screen_size", &json!({}), &[], resolver).is_ok());
            assert!(enforce("list_windows", &json!({"pid": 100}), &[], resolver).is_ok());
            assert!(enforce(
                "launch_app",
                &json!({"bundle_id": "com.apple.TextEdit"}),
                &[],
                resolver
            )
            .is_ok());
            assert!(enforce(
                "launch_app",
                &json!({"urls": ["https://example.com"]}),
                &[],
                resolver
            )
            .is_ok());
        });
    }

    #[test]
    fn a_window_of_the_users_brigadier_is_refused_even_with_another_pid() {
        with_fake_apps(|resolver| {
            let refused = reason_of(enforce(
                "click",
                &json!({"pid": 102, "window_id": 9000}),
                &[],
                resolver,
            ));
            assert_eq!(refused.as_deref(), Some("user_brigadier"));
            let refused = reason_of(enforce("zoom", &json!({"window_id": 9000}), &[], resolver));
            assert_eq!(refused.as_deref(), Some("user_brigadier"));
        });
    }

    #[test]
    fn launching_or_deep_linking_brigadier_is_refused() {
        with_fake_apps(|resolver| {
            for args in [
                json!({"bundle_id": "ai.brigadier.app"}),
                json!({"name": "Brigadier"}),
                json!({"name": "/Applications/brigadier.app"}),
                json!({"urls": ["brigadier://workspace/1"]}),
            ] {
                let refused = reason_of(enforce("launch_app", &args, &[], resolver));
                assert_eq!(refused.as_deref(), Some("user_brigadier"), "{args}");
            }
        });
    }

    #[test]
    fn terminals_driver_and_security_surfaces_are_refused_by_default() {
        with_fake_apps(|resolver| {
            assert_eq!(
                reason_of(enforce("type_text", &json!({"pid": 103}), &[], resolver)).as_deref(),
                Some("terminal")
            );
            assert_eq!(
                reason_of(enforce("click", &json!({"pid": 105}), &[], resolver)).as_deref(),
                Some("security_surface")
            );
            let own = i64::from(std::process::id());
            assert_eq!(
                reason_of(enforce("click", &json!({"pid": own}), &[], resolver)).as_deref(),
                Some("driver")
            );
            assert_eq!(
                reason_of(enforce(
                    "launch_app",
                    &json!({"bundle_id": "ai.brigadier.computer-use"}),
                    &[],
                    resolver
                ))
                .as_deref(),
                Some("driver")
            );
        });
    }

    #[test]
    fn only_an_exact_settings_window_close_passes_the_input_policy() {
        let identity = TargetIdentity {
            name: "Settings".into(),
            bundle_id: Some("systemsettings".into()),
            pid: Some(42),
        };
        let args = json!({"pid":42,"window_id":99,"action":"close"});
        let resolver = TargetResolver {
            app_for_pid: &|_| Some(identity.clone()),
            pid_for_window: &|_| Some(42),
        };
        assert!(enforce("click", &args, &[], &resolver).is_ok());
        for (field, value) in [
            ("x", json!(0)),
            ("element_token", json!("s1:0")),
            ("action", json!("press")),
            ("modifier", json!(["ctrl"])),
        ] {
            let mut modified = args.clone();
            modified[field] = value;
            assert!(
                enforce("click", &modified, &[], &resolver).is_err(),
                "{modified}"
            );
        }
        for id in [
            "consent",
            "credentialuibroker",
            "com.apple.systempreferences",
            "gnome-control-center",
        ] {
            let other = TargetIdentity {
                bundle_id: Some(id.into()),
                name: id.into(),
                pid: Some(42),
            };
            assert!(check_call("click", &args, &other, &[]).is_err(), "{id}");
        }
    }

    #[test]
    fn windows_settings_can_be_terminated_but_other_input_stays_refused() {
        for id in [
            "systemsettings",
            "windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel",
        ] {
            let identity = TargetIdentity {
                name: "Settings".into(),
                bundle_id: Some(id.into()),
                pid: Some(42),
            };
            assert!(check("kill_app", &identity, &[]).is_ok());
            assert!(check("get_window_state", &identity, &[]).is_ok());
            for tool in [
                "click",
                "double_click",
                "right_click",
                "press_key",
                "hotkey",
                "type_text",
                "set_value",
                "scroll",
                "invoke_menu",
            ] {
                assert!(check(tool, &identity, &[]).is_err(), "{tool}");
            }
        }
        for id in [
            "consent",
            "credentialuibroker",
            "regedit",
            "com.apple.systempreferences",
            "gnome-control-center",
        ] {
            let identity = TargetIdentity {
                name: id.into(),
                bundle_id: Some(id.into()),
                pid: Some(43),
            };
            assert!(check("kill_app", &identity, &[]).is_err(), "{id}");
        }
    }

    #[test]
    fn system_settings_can_be_opened_and_read_but_not_changed() {
        with_fake_apps(|resolver| {
            for (tool, args) in [
                (
                    "launch_app",
                    json!({"bundle_id": "com.apple.systempreferences"}),
                ),
                ("launch_app", json!({"name": "System Settings"})),
                ("get_window_state", json!({"pid": 106, "window_id": 9002})),
                ("zoom", json!({"pid": 106})),
            ] {
                assert_eq!(
                    reason_of(enforce(tool, &args, &[], resolver)),
                    None,
                    "{tool}"
                );
            }
            for tool in [
                "click",
                "type_text",
                "set_value",
                "press_key",
                "hotkey",
                "scroll",
            ] {
                let refused = enforce(tool, &json!({"pid": 106}), &[], resolver);
                let text = format!("{refused:?}");
                assert_eq!(
                    reason_of(refused).as_deref(),
                    Some("security_surface"),
                    "{tool}"
                );
                #[cfg(target_os = "macos")]
                let how_to_open = "x-apple.systempreferences:";
                #[cfg(target_os = "windows")]
                let how_to_open = "ms-settings:";
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                let how_to_open = "open them with launch_app";
                assert!(
                    text.contains(how_to_open),
                    "{tool}: the refusal shows how to open a pane"
                );
            }
            assert_eq!(
                reason_of(enforce(
                    "get_window_state",
                    &json!({"pid": 105}),
                    &[],
                    resolver
                ))
                .as_deref(),
                Some("security_surface")
            );
        });
    }

    #[test]
    fn development_builds_are_targetable_and_a_scope_unlocks_only_a_terminal() {
        with_fake_apps(|resolver| {
            assert!(enforce("click", &json!({"pid": 101}), &[], resolver).is_ok());
            // A `tauri dev` build runs bare, without a bundle (#510).
            for tool in ["get_window_state", "click", "page"] {
                assert!(
                    enforce(tool, &json!({"pid": 107}), &[], resolver).is_ok(),
                    "{tool}"
                );
            }
            // No scope reaches the user's Brigadier.
            let tagged = vec!["ai.brigadier.dev.agt1".to_owned()];
            assert_eq!(
                reason_of(enforce("click", &json!({"pid": 100}), &tagged, resolver)).as_deref(),
                Some("user_brigadier")
            );
            // The release family can never be unlocked.
            let release = vec![
                "ai.brigadier.app".to_owned(),
                "ai.brigadier.app.nightly".to_owned(),
            ];
            assert_eq!(
                reason_of(enforce("click", &json!({"pid": 100}), &release, resolver)).as_deref(),
                Some("user_brigadier")
            );
            assert_eq!(
                reason_of(enforce("click", &json!({"pid": 104}), &release, resolver)).as_deref(),
                Some("user_brigadier")
            );
            // Security surfaces cannot be unlocked.
            let keychain = vec!["com.apple.keychainaccess".to_owned()];
            assert_eq!(
                reason_of(enforce("click", &json!({"pid": 105}), &keychain, resolver)).as_deref(),
                Some("security_surface")
            );
            // A terminal can be unlocked by its exact bundle id.
            let terminal = vec!["com.apple.terminal".to_owned()];
            assert!(enforce("click", &json!({"pid": 103}), &terminal, resolver).is_ok());
        });
    }

    #[test]
    fn the_env_form_takes_exact_ids_only() {
        assert_eq!(
            parse_allowed(" ai.brigadier.dev.A, ai.brigadier.app.* \ncom.apple.Terminal"),
            vec![
                "ai.brigadier.dev.a".to_owned(),
                "com.apple.terminal".to_owned()
            ]
        );
    }

    /// On Windows and Linux an app's id is its executable's name.
    #[test]
    fn windows_and_linux_apps_are_named_by_their_executables() {
        let exe = |name: &str| TargetIdentity {
            name: name.to_owned(),
            bundle_id: Some(name.to_ascii_lowercase()),
            pid: Some(42),
        };
        let reason = |name: &str| classify(&exe(name), &[]).map(RefusalReason::as_str);
        for brigadier in ["Brigadier", "brigadier-app", "brigadierd"] {
            assert_eq!(reason(brigadier), Some("user_brigadier"), "{brigadier}");
        }
        assert_eq!(reason("brigadier-cu"), Some("driver"));
        for terminal in [
            "WindowsTerminal",
            "cmd",
            "pwsh",
            "powershell",
            "gnome-terminal-server",
            "konsole",
            "kgx",
            "xterm",
        ] {
            assert_eq!(reason(terminal), Some("terminal"), "{terminal}");
        }
        for prompt in [
            "consent",
            "CredentialUIBroker",
            "LogonUI",
            "polkit-gnome-authentication-agent-1",
            "polkit-kde-authentication-agent-1",
            "gcr-prompter",
            "pinentry-gnome3",
        ] {
            assert_eq!(reason(prompt), Some("security_surface"), "{prompt}");
        }
        for app in ["notepad", "gedit", "firefox", "chrome"] {
            assert_eq!(reason(app), None, "{app}");
        }
        // Windows Terminal launched by its package id.
        assert_eq!(
            classify(
                &TargetIdentity {
                    name: "x".into(),
                    bundle_id: Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App".into()),
                    pid: None,
                },
                &[]
            ),
            Some(RefusalReason::Terminal)
        );
        // A development build (no id) stays targetable; a terminal unlocks
        // by its executable's name.
        let dev = TargetIdentity {
            name: "brigadier-app".into(),
            bundle_id: None,
            pid: Some(7),
        };
        assert_eq!(classify(&dev, &[]), None);
        assert_eq!(classify(&exe("konsole"), &["konsole".to_owned()]), None);
        // The settings apps can be opened and read, not changed.
        for settings in ["SystemSettings", "gnome-control-center"] {
            assert!(check("get_window_state", &exe(settings), &[]).is_ok());
            assert!(check("launch_app", &exe(settings), &[]).is_ok());
            assert!(check("click", &exe(settings), &[]).is_err());
        }
    }

    #[test]
    fn native_launch_aliases_cannot_launch_protected_executables() {
        for args in [
            json!({"path": "C:\\Windows\\System32\\cmd.exe"}),
            json!({"launch_path": "\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\" -Command whoami"}),
            json!({"aumid": "Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"}),
            json!({"launch_path": "shell:appsFolder\\Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"}),
            json!({"name": "/usr/bin/gnome-terminal --wait"}),
            json!({"launch_path": "/usr/bin/bash -c whoami"}),
            json!({"launch_path": "/usr/bin/env VAR=value /usr/bin/bash -c whoami"}),
            json!({"name": "env", "additional_arguments":["bash","-c","whoami"]}),
            json!({"launch_path": "/opt/Brigadier/brigadier-app"}),
            json!({"launch_path": "polkit-gnome-authentication-agent-1"}),
        ] {
            assert!(
                enforce_executable_launch("launch_app", &args, &[]).is_err(),
                "{args}"
            );
        }
        for args in [
            json!({"path":"C:\\Windows\\notepad.exe"}),
            json!({"launch_path":"gedit --new-window"}),
            json!({"launch_path":"env GDK_BACKEND=x11 /usr/bin/gedit --new-window"}),
        ] {
            assert!(enforce_executable_launch("launch_app", &args, &[]).is_ok());
        }
        for settings in [
            "systemsettings5",
            "systemsettings6",
            "xfce4-settings-manager",
            "control",
            "regedit",
        ] {
            let identity = TargetIdentity {
                name: settings.into(),
                bundle_id: Some(settings.into()),
                pid: Some(42),
            };
            assert!(check("click", &identity, &[]).is_err(), "{settings}");
            assert!(check("type_text", &identity, &[]).is_err(), "{settings}");
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn native_targets_fail_closed_when_the_process_cannot_be_identified() {
        let resolver = TargetResolver {
            app_for_pid: &|_| None,
            pid_for_window: &|_| None,
        };
        for args in [json!({"pid":4242}), json!({"window_id":123})] {
            let error = enforce("click", &args, &[], &resolver).unwrap_err();
            assert_eq!(
                error.structured_content.unwrap()["code"],
                "target_identity_unavailable"
            );
            assert!(enforce("list_windows", &args, &[], &resolver).is_ok());
        }
    }
}
