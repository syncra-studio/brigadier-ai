//! Notification permission: whether Brigadier may show notifications, asking for it, and the
//! system's settings for it (Settings › General).

use std::time::Duration;

use tauri::AppHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
// Only macOS answers anything but Unknown.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum Permission {
    /// Not a bundled macOS app, or another platform: nothing to say.
    Unknown,
    NotAsked,
    Off,
    Allowed,
}

pub async fn permission() -> Permission {
    #[cfg(target_os = "macos")]
    return mac::permission().await;
    #[cfg(not(target_os = "macos"))]
    Permission::Unknown
}

/// Opens System Settings at Brigadier's notifications.
pub fn open_settings(app: &AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_opener::OpenerExt;
        let url = format!(
            "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={}",
            app.config().identifier
        );
        app.opener()
            .open_url(url, None::<&str>)
            .map_err(|err| err.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("Open your system's notification settings to turn them on for Brigadier.".into())
    }
}

/// Asks for notification permission (Settings › General) and waits for the answer, up to a
/// minute; elsewhere there is nothing to ask.
pub async fn ask_and_wait() -> Permission {
    #[cfg(target_os = "macos")]
    mac::ask();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let now = permission().await;
        if now != Permission::NotAsked || tokio::time::Instant::now() >= deadline {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

// A narrow FFI boundary: Objective-C owns UNUserNotificationCenter; Rust owns callback
// tickets. No pointer survives a callback.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::oneshot;

    static PERMITTED: OnceLock<Mutex<HashMap<u64, oneshot::Sender<i32>>>> = OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(1);

    unsafe extern "C" {
        fn brigadier_notice_ask();
        fn brigadier_notice_permission(ticket: u64, callback: extern "C" fn(u64, i32));
    }
    extern "C" fn permitted(ticket: u64, answer: i32) {
        if let Some(sender) = PERMITTED
            .get()
            .and_then(|pending| pending.lock().ok()?.remove(&ticket))
        {
            let _ = sender.send(answer);
        }
    }
    pub async fn permission() -> Permission {
        let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let pending = PERMITTED.get_or_init(|| Mutex::new(HashMap::new()));
        match pending.lock() {
            Ok(mut waiting) => waiting.insert(ticket, sender),
            Err(_) => return Permission::Unknown,
        };
        // SAFETY: a static callback that resolves only its own u64 ticket.
        unsafe { brigadier_notice_permission(ticket, permitted) };
        let answer = tokio::time::timeout(Duration::from_secs(5), receiver).await;
        if let Ok(mut waiting) = pending.lock() {
            waiting.remove(&ticket);
        }
        match answer {
            Ok(Ok(0)) => Permission::NotAsked,
            Ok(Ok(1)) => Permission::Off,
            Ok(Ok(2)) => Permission::Allowed,
            _ => Permission::Unknown,
        }
    }
    pub fn ask() {
        // SAFETY: takes no arguments; the completion handler is owned by ObjC.
        unsafe { brigadier_notice_ask() };
    }
}
