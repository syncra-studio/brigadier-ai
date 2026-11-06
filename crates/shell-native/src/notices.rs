//! What only native code can read for the Electron shell's drops and notifications: whether Shift is
//! held, and notification permission, as the Tauri crate's `drops` and `notifications` have them.

use napi_derive::napi;

/// Whether Shift is held right now.
#[napi]
pub fn shift_held() -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSEvent, NSEventModifierFlags};
        NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Shift)
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// `unknown` (not a bundled macOS app, or another platform), `notAsked`, `off` or `allowed`.
#[napi]
pub async fn notice_permission() -> String {
    #[cfg(target_os = "macos")]
    return mac::permission().await.to_owned();
    #[cfg(not(target_os = "macos"))]
    "unknown".to_owned()
}

/// Asks the system to let Brigadier notify. Its answer shows in `notice_permission`.
#[napi]
pub fn notice_ask() {
    #[cfg(target_os = "macos")]
    mac::ask();
}

// A narrow FFI boundary: Objective-C owns UNUserNotificationCenter; Rust owns callback tickets. No
// pointer survives a callback.
#[cfg(target_os = "macos")]
mod mac {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

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

    pub async fn permission() -> &'static str {
        let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let pending = PERMITTED.get_or_init(|| Mutex::new(HashMap::new()));
        match pending.lock() {
            Ok(mut waiting) => waiting.insert(ticket, sender),
            Err(_) => return "unknown",
        };
        // SAFETY: a static callback that resolves only its own u64 ticket.
        unsafe { brigadier_notice_permission(ticket, permitted) };
        let answer = tokio::time::timeout(Duration::from_secs(5), receiver).await;
        if let Ok(mut waiting) = pending.lock() {
            waiting.remove(&ticket);
        }
        match answer {
            Ok(Ok(0)) => "notAsked",
            Ok(Ok(1)) => "off",
            Ok(Ok(2)) => "allowed",
            _ => "unknown",
        }
    }

    pub fn ask() {
        // SAFETY: takes no arguments; the completion handler is owned by ObjC.
        unsafe { brigadier_notice_ask() };
    }
}
