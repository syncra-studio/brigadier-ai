//! Computer use's setup, from the shell: System Settings at the right privacy list.

use brigadier_core::ComputerPermission;
use brigadier_ipc::protocol::{ErrorCode, IpcError};
use tauri_plugin_opener::OpenerExt;

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

/// Opens System Settings at the list that grants `permission`.
#[tauri::command]
pub fn open_privacy_settings(
    app: tauri::AppHandle,
    permission: ComputerPermission,
) -> Result<(), IpcError> {
    app.opener()
        .open_url(privacy_pane(permission), None::<&str>)
        .map_err(|err| IpcError {
            code: ErrorCode::Internal,
            message: format!("couldn't open System Settings: {err}"),
        })
}
