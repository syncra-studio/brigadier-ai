//! Open at login (Settings › General › Power): the system's own way of starting an app when
//! the user logs in. macOS: the app's login item (SMAppService), for the bundled app only.
//! Windows: the user's `Run` registry key. Linux: an XDG autostart entry. The system keeps the
//! state, not Brigadier's settings, so it is read from there each time.

use tauri::AppHandle;

/// Whether Brigadier opens at login, as the system says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LoginItem {
    /// Not here: a development binary on macOS, or a system without a way.
    Unsupported,
    Off,
    On,
    /// Turned on, but macOS waits for the user to allow it in System Settings › Login Items.
    #[cfg(target_os = "macos")]
    NeedsApproval,
}

#[tauri::command]
pub fn login_item(app: AppHandle) -> LoginItem {
    imp::status(&app)
}

/// Turns opening at login on or off, then answers how it stands.
#[tauri::command]
pub fn set_login_item(app: AppHandle, on: bool) -> Result<LoginItem, String> {
    imp::set(&app, on)?;
    Ok(imp::status(&app))
}

/// Opens the system's list of login items (macOS asks the user to allow Brigadier there).
#[tauri::command]
pub fn open_login_items() -> Result<(), String> {
    imp::open_settings()
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod imp {
    use super::*;
    use std::ffi::{CStr, c_char};

    unsafe extern "C" {
        fn brigadier_login_item_status() -> i32;
        fn brigadier_login_item_set(on: bool) -> *mut c_char;
        fn brigadier_login_item_free(reason: *mut c_char);
        fn brigadier_login_item_open_settings();
    }

    /// Only an app bundle can be a login item; a bare development binary can't.
    fn bundled() -> bool {
        std::env::current_exe().ok().is_some_and(|exe| {
            exe.parent()
                .and_then(|path| path.parent())
                .is_some_and(|path| path.file_name().is_some_and(|name| name == "Contents"))
        })
    }

    pub fn status(_app: &AppHandle) -> LoginItem {
        if !bundled() {
            return LoginItem::Unsupported;
        }
        // SAFETY: takes nothing and returns a plain integer.
        match unsafe { brigadier_login_item_status() } {
            0 => LoginItem::Off,
            1 => LoginItem::On,
            2 => LoginItem::NeedsApproval,
            _ => LoginItem::Unsupported,
        }
    }

    pub fn set(app: &AppHandle, on: bool) -> Result<(), String> {
        if !bundled() {
            return Err("Only the installed app can open at login.".into());
        }
        // Unregistering what isn't registered is an error to SMAppService; nothing to do.
        if !on && status(app) == LoginItem::Off {
            return Ok(());
        }
        // SAFETY: the adapter returns null or a strdup'd C string, freed here once copied.
        unsafe {
            let reason = brigadier_login_item_set(on);
            if reason.is_null() {
                return Ok(());
            }
            let text = CStr::from_ptr(reason).to_string_lossy().into_owned();
            brigadier_login_item_free(reason);
            Err(text)
        }
    }

    pub fn open_settings() -> Result<(), String> {
        // SAFETY: takes nothing; AppKit opens System Settings.
        unsafe { brigadier_login_item_open_settings() };
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    /// No console window flashes up for `reg`.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn name(app: &AppHandle) -> String {
        app.config()
            .product_name
            .clone()
            .unwrap_or_else(|| "Brigadier".to_owned())
    }

    fn reg(args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("reg")
            .args(args)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
    }

    pub fn status(app: &AppHandle) -> LoginItem {
        match reg(&["query", RUN_KEY, "/v", &name(app)]) {
            Ok(output) if output.status.success() => LoginItem::On,
            Ok(_) => LoginItem::Off,
            Err(_) => LoginItem::Unsupported,
        }
    }

    pub fn set(app: &AppHandle, on: bool) -> Result<(), String> {
        let name = name(app);
        let output = if on {
            let exe = std::env::current_exe().map_err(|err| err.to_string())?;
            let command = format!("\"{}\"", exe.display());
            reg(&[
                "add", RUN_KEY, "/v", &name, "/t", "REG_SZ", "/d", &command, "/f",
            ])
        } else {
            if status(app) == LoginItem::Off {
                return Ok(());
            }
            reg(&["delete", RUN_KEY, "/v", &name, "/f"])
        }
        .map_err(|err| err.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    }

    pub fn open_settings() -> Result<(), String> {
        Err("Startup apps are in Settings › Apps › Startup.".into())
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use std::path::PathBuf;

    /// `$XDG_CONFIG_HOME/autostart`, else `~/.config/autostart`.
    fn autostart_dir() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(config.join("autostart"))
    }

    fn entry(app: &AppHandle) -> Option<PathBuf> {
        Some(autostart_dir()?.join(format!("{}.desktop", app.config().identifier)))
    }

    pub fn status(app: &AppHandle) -> LoginItem {
        match entry(app) {
            Some(path) if path.is_file() => LoginItem::On,
            Some(_) => LoginItem::Off,
            None => LoginItem::Unsupported,
        }
    }

    pub fn set(app: &AppHandle, on: bool) -> Result<(), String> {
        let path = entry(app).ok_or("No home folder to keep the autostart entry in.")?;
        if !on {
            return match std::fs::remove_file(&path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err.to_string()),
                _ => Ok(()),
            };
        }
        // An AppImage runs from a temporary mount: start the image itself.
        let exe = match std::env::var_os("APPIMAGE") {
            Some(image) => PathBuf::from(image),
            None => std::env::current_exe().map_err(|err| err.to_string())?,
        };
        let name = app
            .config()
            .product_name
            .clone()
            .unwrap_or_else(|| "Brigadier".to_owned());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
        }
        std::fs::write(&path, super::desktop_entry(&name, &exe.to_string_lossy()))
            .map_err(|err| err.to_string())
    }

    pub fn open_settings() -> Result<(), String> {
        Err("Startup applications are in your desktop's settings.".into())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod imp {
    use super::*;

    pub fn status(_app: &AppHandle) -> LoginItem {
        LoginItem::Unsupported
    }

    pub fn set(_app: &AppHandle, _on: bool) -> Result<(), String> {
        Err("Opening at login isn't available on this system.".into())
    }

    pub fn open_settings() -> Result<(), String> {
        Err("Opening at login isn't available on this system.".into())
    }
}

/// An XDG autostart entry that starts `exe`.
#[cfg(any(target_os = "linux", test))]
fn desktop_entry(name: &str, exe: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={}\nX-GNOME-Autostart-enabled=true\n",
        exec_arg(exe)
    )
}

/// One argument of a desktop entry's `Exec`, quoted as the Desktop Entry spec says: inside
/// double quotes `"`, `` ` ``, `$` and `\` take a backslash, a literal `%` is `%%`, and then the
/// value's own string escaping doubles every backslash.
#[cfg(any(target_os = "linux", test))]
fn exec_arg(arg: &str) -> String {
    let mut quoted = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_paths_are_quoted_for_desktop_entries() {
        assert_eq!(
            exec_arg("/opt/Brigadier/brigadier-app"),
            "\"/opt/Brigadier/brigadier-app\""
        );
        assert_eq!(
            exec_arg("/home/a b/Brigadier.AppImage"),
            "\"/home/a b/Brigadier.AppImage\""
        );
        assert_eq!(exec_arg("/tmp/$x`y\"%"), "\"/tmp/\\\\$x\\\\`y\\\\\"%%\"");
        assert_eq!(exec_arg("/tmp/a\\b"), "\"/tmp/a\\\\\\\\b\"");
        let entry = desktop_entry("Brigadier", "/opt/b");
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\nName=Brigadier\n"));
        assert!(entry.contains("\nExec=\"/opt/b\"\n"));
    }
}
