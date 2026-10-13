//! Computer use's setup, from the shell: the setup window (three steps: Accessibility, Screen
//! Recording, a test picture), System Settings at the right privacy list, and the helper app's
//! card the user drags into that list.
//!
//! The setup window is a second webview on the same frontend (`?window=computer-setup`). It
//! gets computer use's status over its own channel ([`computer_setup_subscribe`]), so opening
//! and closing it leaves the main window's feed alone.

use brigadier_core::ComputerPermission;
use brigadier_ipc::app::BridgeEvent;
use brigadier_ipc::protocol::{ErrorCode, IpcError};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

use crate::AppState;

/// The setup window's label.
pub const SETUP_WINDOW: &str = "computer-setup";
/// The helper app's bundle name in the data folder, where it runs from.
#[cfg(target_os = "macos")]
const HELPER_APP: &str = "Brigadier Computer Use.app";

/// System Settings' privacy list that grants `permission`.
fn privacy_pane(permission: ComputerPermission) -> &'static str {
    match permission {
        ComputerPermission::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        ComputerPermission::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
    }
}

fn internal(message: String) -> IpcError {
    IpcError {
        code: ErrorCode::Internal,
        message,
    }
}

/// Opens System Settings at the list that grants `permission`.
#[tauri::command]
pub fn open_privacy_settings(
    app: AppHandle,
    permission: ComputerPermission,
) -> Result<(), IpcError> {
    app.opener()
        .open_url(privacy_pane(permission), None::<&str>)
        .map_err(|err| internal(format!("couldn't open System Settings: {err}")))
}

/// Opens the setup window, or brings it forward. `focus`: it comes to the front (the user asked
/// for it); opened for an agent, it shows without taking the keyboard from what the user does.
pub fn open(app: &AppHandle, focus: bool) {
    if let Some(window) = app.get_webview_window(SETUP_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        if focus {
            let _ = window.set_focus();
        }
        return;
    }
    let built = WebviewWindowBuilder::new(
        app,
        SETUP_WINDOW,
        WebviewUrl::App("index.html?window=computer-setup".into()),
    )
    .title("Set up computer use")
    .inner_size(560.0, 600.0)
    .resizable(false)
    .maximizable(false)
    .theme(Some(tauri::Theme::Dark))
    .background_color(tauri::window::Color(0x0f, 0x0f, 0x0e, 0xff))
    // The helper's card is dragged out natively; nothing is dropped in.
    .disable_drag_drop_handler()
    .focused(focus)
    .build();
    if let Err(err) = built {
        tracing::warn!(%err, "couldn't open the computer-use setup window");
    }
}

/// Settings › Computer use › Finish setup….
#[tauri::command]
pub fn open_computer_setup(app: AppHandle) {
    open(&app, true);
}

/// The setup window's feed: computer use's status as it changes.
#[tauri::command]
pub fn computer_setup_subscribe(state: State<'_, AppState>, channel: Channel<BridgeEvent>) {
    state.bridge.attach_setup_ui(channel);
}

/// The placed helper app, which System Settings lists.
#[cfg(target_os = "macos")]
fn helper_app(state: &AppState) -> Result<std::path::PathBuf, IpcError> {
    let app = state.bridge.data_dir().join("computer").join(HELPER_APP);
    if app.exists() {
        Ok(app)
    } else {
        Err(internal(
            "the computer-use helper isn't ready yet; try again in a moment".into(),
        ))
    }
}

/// Starts dragging the helper app from under the pointer in the setup window, while the mouse
/// button is held: dropped into the list in System Settings, it is added there.
#[tauri::command]
pub fn computer_setup_drag(app: AppHandle, state: State<'_, AppState>) -> Result<(), IpcError> {
    #[cfg(target_os = "macos")]
    {
        let path = helper_app(&state)?;
        let window = app
            .get_webview_window(SETUP_WINDOW)
            .ok_or_else(|| internal("the setup window is closed".into()))?;
        let target = window.clone();
        window
            .run_on_main_thread(move || {
                if let Ok(ns_window) = target.ns_window()
                    && !drag::file(ns_window, &path)
                {
                    tracing::warn!("couldn't start dragging the computer-use helper");
                }
            })
            .map_err(|err| internal(format!("couldn't start the drag: {err}")))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
        Err(internal("computer use needs macOS".into()))
    }
}

/// Shows the helper app in Finder, to drag from there.
#[tauri::command]
pub fn reveal_computer_helper(app: AppHandle, state: State<'_, AppState>) -> Result<(), IpcError> {
    #[cfg(target_os = "macos")]
    {
        let path = helper_app(&state)?;
        app.opener()
            .reveal_item_in_dir(path)
            .map_err(|err| internal(format!("couldn't show it: {err}")))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, state);
        Err(internal("computer use needs macOS".into()))
    }
}

/// The drag itself, in AppKit (`computer_setup_drag.m`): the shell's narrow unsafe exception
/// for it, as for the other Objective-C parts.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod drag {
    use std::ffi::{CString, c_char, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    unsafe extern "C" {
        fn brigadier_drag_file(ns_window: *mut c_void, path: *const c_char) -> i32;
    }

    /// Drags `path` from `ns_window`. Main thread only.
    pub fn file(ns_window: *mut c_void, path: &Path) -> bool {
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: `ns_window` is the live NSWindow of a window this thread (the main thread)
        // owns, and `path` a NUL-terminated string that outlives the call; the function keeps
        // neither.
        unsafe { brigadier_drag_file(ns_window, path.as_ptr()) == 1 }
    }
}
