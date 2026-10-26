//! What this desktop session lets the helper do on Windows and Linux, in
//! place of macOS's permission grants, and the running app for an app id.
//!
//! Windows asks for no permissions: the helper reaches every app of the
//! user's session through UI Automation and posts input in the background.
//! An app running elevated, and packaged (UWP) apps, are out of reach for a
//! helper that isn't elevated itself; the driver says so when it meets one.
//!
//! Linux depends on the session. X11, and X11 apps under XWayland, work
//! through AT-SPI and XTest. A pure Wayland session (no XWayland `DISPLAY`)
//! can't take background input unless the driver's experimental Wayland
//! backend is switched on, and capture there goes through the desktop
//! portal. The notes say which case this is, for Brigadier's settings page.

use serde_json::{json, Value};

/// The facts Brigadier shows and the daemon reports as permissions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Apps' accessibility trees can be read and their controls used.
    pub accessibility: bool,
    /// Windows can be captured.
    pub screen_capture: bool,
    /// `windows`, `x11`, `wayland` or `none`.
    pub display_server: &'static str,
    /// Plain sentences for the user about what works here and what doesn't.
    pub notes: Vec<String>,
}

impl Facts {
    pub fn to_json(&self) -> Value {
        json!({
            "os": std::env::consts::OS,
            "display_server": self.display_server,
            "accessibility": self.accessibility,
            "screen_capture": self.screen_capture,
            "notes": self.notes,
        })
    }
}

/// The facts of this session.
#[cfg(target_os = "windows")]
pub fn facts() -> Facts {
    Facts {
        accessibility: true,
        screen_capture: true,
        display_server: "windows",
        notes: vec![
            "Windows asks for no permissions: agents use apps in the background through UI \
             Automation."
                .to_owned(),
            "Apps running as administrator and Store (UWP) apps can't be driven in the \
             background."
                .to_owned(),
        ],
    }
}

/// The facts of this session, read from its environment.
#[cfg(target_os = "linux")]
pub fn facts() -> Facts {
    let environment = |key: &str| std::env::var_os(key).filter(|value| !value.is_empty());
    linux_facts(
        environment("DISPLAY").is_some(),
        environment("WAYLAND_DISPLAY").is_some(),
        platform_linux::wayland::wayland_enabled(),
        environment("NO_AT_BRIDGE").as_deref() == Some(std::ffi::OsStr::new("1")),
    )
}

/// [`facts`] from what the Linux session has: an X11 display (XWayland
/// counts), a Wayland display, the driver's Wayland backend switched on,
/// and the accessibility bridge switched off.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_facts(x11: bool, wayland: bool, wayland_backend: bool, no_bridge: bool) -> Facts {
    let mut notes = Vec::new();
    let display_server = match (x11, wayland) {
        (_, true) => "wayland",
        (true, false) => "x11",
        (false, false) => "none",
    };
    if !x11 && !wayland {
        notes.push(
            "No desktop session was found (no DISPLAY or WAYLAND_DISPLAY): agents can't see \
             or use apps."
                .to_owned(),
        );
    } else if wayland && x11 && !wayland_backend {
        notes.push(
            "Wayland session: apps running under XWayland work in the background; native \
             Wayland apps can be read but not always captured or typed into."
                .to_owned(),
        );
    } else if wayland && !x11 && !wayland_backend {
        notes.push(
            "Pure Wayland session without XWayland: agents can read apps but can't type or \
             click in the background."
                .to_owned(),
        );
    } else if wayland && wayland_backend {
        notes.push(
            "Wayland session with the driver's experimental Wayland backend: works on \
             wlroots compositors (sway, labwc, Hyprland); GNOME and KDE capture goes \
             through the desktop portal."
                .to_owned(),
        );
    }
    if no_bridge {
        notes.push(
            "The accessibility bridge is switched off (NO_AT_BRIDGE=1): apps' controls \
             can't be read."
                .to_owned(),
        );
    }
    Facts {
        accessibility: (x11 || wayland) && !no_bridge,
        screen_capture: x11 || wayland,
        display_server,
        notes,
    }
}

#[cfg(not(target_os = "macos"))]
/// The running process of the app whose id is `app_id`: the lower-cased
/// executable name without `.exe` ([`crate::lookup`]). Development builds
/// have no id and are never found here.
pub fn running_pid(app_id: &str) -> Option<i64> {
    use crate::lookup::Lookups;
    let wanted = app_id.to_ascii_lowercase();
    process_ids()
        .into_iter()
        .filter(|pid| *pid != std::process::id())
        .find(|pid| {
            crate::lookup::System
                .app_for_pid(i64::from(*pid))
                .and_then(|identity| identity.bundle_id)
                .is_some_and(|id| id == wanted)
        })
        .map(i64::from)
}

/// Every process id visible to this user.
#[cfg(target_os = "linux")]
fn process_ids() -> Vec<u32> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect()
}

#[cfg(target_os = "windows")]
fn process_ids() -> Vec<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut pids = Vec::new();
    // SAFETY: the snapshot handle is checked and closed; the entry's size is
    // set before the first call, as the API requires.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return pids;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                pids.push(entry.th32ProcessID);
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    pids
}

/// A refusal this platform can never get past in the background, in one
/// plain sentence ahead of the driver's own text (also as
/// `structuredContent.platform_note`): an app running as administrator or a
/// Store app on Windows, input in a pure Wayland session on Linux. Other
/// results pass unchanged.
pub fn explain(
    mut result: cua_driver_core::protocol::ToolResult,
) -> cua_driver_core::protocol::ToolResult {
    use cua_driver_core::protocol::Content;
    if result.is_error != Some(true) {
        return result;
    }
    let code = result
        .structured_content
        .as_ref()
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let text: String = result
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let Some(note) = platform_note(&code, &text) else {
        return result;
    };
    match result
        .content
        .iter_mut()
        .find(|content| matches!(content, Content::Text { .. }))
    {
        Some(Content::Text { text, .. }) => *text = format!("{note} {text}"),
        _ => result.content.insert(
            0,
            Content::Text {
                text: note.to_owned(),
                annotations: None,
            },
        ),
    }
    if let Some(Value::Object(structured)) = result.structured_content.as_mut() {
        structured.insert("platform_note".to_owned(), Value::from(note));
    }
    result
}

/// The sentence for a refusal `code` with driver `text`, if it is one this
/// platform can't get past.
fn platform_note(code: &str, text: &str) -> Option<&'static str> {
    if code == "background_uipi_blocked" || text.contains("blocked by UIPI") {
        return Some(
            "Windows keeps this app out of reach: it runs as administrator or is a Store \
             (UWP) app, and Brigadier's computer use can't drive such apps in the background. \
             Ask the user to do this step.",
        );
    }
    if text.contains("pure Wayland session") {
        return Some(
            "This Linux desktop is a pure Wayland session, where Brigadier's computer use \
             can read apps but can't type or click in them. Ask the user to do this step, \
             or to run the app under XWayland.",
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::linux_facts;

    #[test]
    fn a_linux_session_says_what_works() {
        let x11 = linux_facts(true, false, false, false);
        assert_eq!(x11.display_server, "x11");
        assert!(x11.accessibility && x11.screen_capture);
        assert!(x11.notes.is_empty());

        let xwayland = linux_facts(true, true, false, false);
        assert_eq!(xwayland.display_server, "wayland");
        assert!(xwayland.notes[0].contains("XWayland"));

        let pure = linux_facts(false, true, false, false);
        assert!(pure.notes[0].contains("can't type or click"));

        let none = linux_facts(false, false, false, false);
        assert_eq!(none.display_server, "none");
        assert!(!none.accessibility && !none.screen_capture);

        let no_bridge = linux_facts(true, false, false, true);
        assert!(!no_bridge.accessibility);
        assert!(no_bridge.notes[0].contains("NO_AT_BRIDGE"));
    }

    #[test]
    fn a_refusal_the_platform_cant_get_past_says_so_plainly() {
        use cua_driver_core::protocol::{Content, ToolResult};
        let uipi =
            ToolResult::error("click was not delivered: blocked by UIPI higher-integrity target")
                .with_structured(serde_json::json!({ "code": "background_uipi_blocked" }));
        let explained = super::explain(uipi);
        let Content::Text { text, .. } = &explained.content[0] else {
            panic!("text first");
        };
        assert!(
            text.starts_with("Windows keeps this app out of reach"),
            "{text}"
        );
        assert!(text.ends_with("higher-integrity target"), "{text}");
        assert!(explained.structured_content.unwrap()["platform_note"]
            .as_str()
            .unwrap()
            .contains("administrator"));

        let wayland =
            ToolResult::error("input cannot be delivered: pure Wayland session (no X11 DISPLAY)");
        let Content::Text { text, .. } = &super::explain(wayland).content[0] else {
            panic!("text first");
        };
        assert!(text.contains("pure Wayland session, where"), "{text}");

        let other = ToolResult::error("element_token is stale");
        let Content::Text { text, .. } = &super::explain(other).content[0] else {
            panic!("text first");
        };
        assert_eq!(text, "element_token is stale");
    }
}
