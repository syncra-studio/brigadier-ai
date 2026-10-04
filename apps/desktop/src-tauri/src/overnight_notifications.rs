//! Durable run-notification delivery, as Brigadier, including a hidden app invocation.
//! Pending is read after connecting, not inferred from the live subscription's head.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use brigadier_core::overnight::NOTIFICATIONS_OFF;
use brigadier_ipc::protocol::{PendingRunNotification, Request, Response};
use tauri::{AppHandle, Manager};

static HOST: AtomicBool = AtomicBool::new(false);

pub fn is_host() -> bool {
    HOST.load(Ordering::Acquire)
}

pub fn host_id(args: &[String]) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == "--overnight-notification")
        .map(|pair| pair[1].clone())
}

/// Applied before platform paths/single-instance setup, so a host uses the exact daemon data.
pub fn configure(args: &[String]) {
    HOST.store(host_id(args).is_some(), Ordering::Release);
}

/// The user opened Brigadier (a launch, the Dock): a hidden notification host becomes the
/// ordinary app, so its window stays shown and quitting it behaves as usual.
pub fn leave_host() {
    HOST.store(false, Ordering::Release);
}

/// The bundle remembers its own last data directory for activation after a complete exit.
/// Explicit CLI/environment paths take precedence; bare development binaries remember none.
#[cfg(target_os = "macos")]
pub fn activation_data_dir() -> Option<std::path::PathBuf> {
    mac::data_dir()
}

pub fn install(app: &AppHandle, intent: Option<String>) {
    #[cfg(target_os = "macos")]
    mac::install(app.clone());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut validated = intent.is_none();
        let mut asked = false;
        loop {
            let Some(state) = app.try_state::<crate::AppState>() else {
                return;
            };
            let bridge = state.bridge.clone();
            if let Ok(Response::PendingOvernightNotifications { notifications }) =
                bridge.request(Request::PendingOvernightNotifications).await
            {
                // This authenticated response validates the host's intent; arbitrary CLI IDs
                // cannot manufacture notices or change their target session.
                if !validated {
                    validated = notifications
                        .iter()
                        .any(|pending| intent.as_ref() == Some(&pending.notification.id));
                }
                let notifications: Vec<_> =
                    notifications.into_iter().filter(|_| validated).collect();
                let permission = if notifications.is_empty() {
                    Permission::Unknown
                } else {
                    permission().await
                };
                if !notifications.is_empty() {
                    tracing::debug!(
                        ?permission,
                        pending = notifications.len(),
                        "run notifications to show"
                    );
                }
                for pending in notifications {
                    match permission {
                        // Says so once, and tries again only once they're back on: submitting
                        // would only be refused again.
                        Permission::Off => {
                            if pending.notification.delivery_error.as_deref()
                                != Some(NOTIFICATIONS_OFF)
                            {
                                tracing::info!(notification = %pending.notification.id, "notifications are off; the run notification waits");
                                fail(&bridge, pending, NOTIFICATIONS_OFF.to_owned()).await;
                            }
                        }
                        // Asks once and waits for the answer; Start normally asked already.
                        Permission::NotAsked => {
                            if !asked {
                                asked = true;
                                ask_permission();
                            }
                        }
                        Permission::Allowed | Permission::Unknown => {
                            match submit(&app, &pending).await {
                                Ok(()) => {
                                    tracing::info!(notification = %pending.notification.id, "run notification submitted as Brigadier");
                                    if let Err(err) = bridge
                                        .request(Request::AckOvernightNotification {
                                            conversation_id: pending.conversation_id,
                                            run_id: pending.run_id,
                                            notification_id: pending.notification.id,
                                        })
                                        .await
                                    {
                                        tracing::warn!(error = ?err, "run notification acknowledgement failed");
                                    }
                                }
                                Err(err) => {
                                    tracing::warn!(error = %err, "run notification was not submitted; report retained");
                                    fail(&bridge, pending, err).await;
                                }
                            }
                        }
                    }
                }
                if intent.is_some() && is_host() {
                    // Keep a small hidden app resident to receive activation, including a
                    // retry of an OS permission prompt. The daemon owns no UI process lifetime.
                    crate::shell::hide_main(&app);
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

/// Records why a run's notification wasn't shown; it stays pending.
async fn fail(bridge: &crate::bridge::Bridge, pending: PendingRunNotification, error: String) {
    let _ = bridge
        .request(Request::FailOvernightNotification {
            conversation_id: pending.conversation_id,
            run_id: pending.run_id,
            notification_id: pending.notification.id,
            error,
        })
        .await;
}

/// Whether Brigadier may show notifications, as the OS says without asking.
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

/// Starting a run asks for notification permission, while the user is at the Mac: its
/// report notification may come hours later, with nobody there to answer a prompt.
pub fn ask_permission() {
    #[cfg(target_os = "macos")]
    mac::ask();
}

#[cfg(target_os = "macos")]
async fn submit(_app: &AppHandle, notice: &PendingRunNotification) -> Result<(), String> {
    mac::submit(notice).await
}

#[cfg(not(target_os = "macos"))]
async fn submit(app: &AppHandle, notice: &PendingRunNotification) -> Result<(), String> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title(&notice.notification.title)
        .body(&notice.notification.body)
        .show()
        .map_err(|err| err.to_string())
}

/// Activation is checked against the stored run before opening its existing session.
#[cfg(target_os = "macos")]
fn activate(app: &AppHandle, conversation: String, run: String, notification: String) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(state) = app.try_state::<crate::AppState>() else {
            return;
        };
        let bridge = state.bridge.clone();
        if let Ok(Response::GetConversation { view }) = bridge
            .request(Request::GetConversation {
                id: brigadier_core::ConversationId(conversation.clone()),
                limit: 100,
            })
            .await
        {
            let Some(record) = view.overnight.iter().find(|record| {
                record.id.0 == run
                    && record
                        .notification
                        .as_ref()
                        .is_some_and(|notice| notice.id == notification)
            }) else {
                return;
            };
            if let Some(head) = &record.report_message_id {
                let _ = bridge
                    .request(Request::SwitchBranch {
                        conversation_id: brigadier_core::ConversationId(conversation.clone()),
                        head: head.clone(),
                    })
                    .await;
            }
            HOST.store(false, Ordering::Release);
            let target = app.clone();
            let _ = app.run_on_main_thread(move || crate::shell::show_main(&target));
            bridge.open_conversation(conversation);
        }
    });
}

// A narrow FFI boundary: Objective-C owns UNUserNotificationCenter and all copied strings;
// Rust owns callback tickets and the app handle. No pointer survives a callback.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::{CStr, CString, c_char};
    use std::sync::atomic::AtomicU64;
    use std::sync::{Mutex, OnceLock};
    use tokio::sync::oneshot;

    static APP: OnceLock<AppHandle> = OnceLock::new();
    type Submitted = oneshot::Sender<Result<(), String>>;
    static PENDING: OnceLock<Mutex<HashMap<u64, Submitted>>> = OnceLock::new();
    static PERMITTED: OnceLock<Mutex<HashMap<u64, oneshot::Sender<i32>>>> = OnceLock::new();
    /// What the adapter reports for a refusal because notifications are off.
    const OFF: &str = "notifications-off";
    static NEXT: AtomicU64 = AtomicU64::new(1);

    unsafe extern "C" {
        fn brigadier_notice_init(
            callback: extern "C" fn(*const c_char, *const c_char, *const c_char),
            data_dir: *const c_char,
        );
        fn brigadier_notice_data_dir() -> *const c_char;
        fn brigadier_notice_ask();
        fn brigadier_notice_permission(ticket: u64, callback: extern "C" fn(u64, i32));
        fn brigadier_notice_send(
            identifier: *const c_char,
            title: *const c_char,
            body: *const c_char,
            conversation: *const c_char,
            run: *const c_char,
            ticket: u64,
            callback: extern "C" fn(u64, *const c_char),
        );
    }
    fn string(ptr: *const c_char) -> String {
        // SAFETY: the adapter calls back with a live UTF-8 NSString C string for this call.
        unsafe { CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned()
    }
    extern "C" fn activated(
        conversation: *const c_char,
        run: *const c_char,
        notice: *const c_char,
    ) {
        if let Some(app) = APP.get() {
            super::activate(app, string(conversation), string(run), string(notice));
        }
    }
    extern "C" fn submitted(ticket: u64, error: *const c_char) {
        if let Some(sender) = PENDING
            .get()
            .and_then(|pending| pending.lock().ok()?.remove(&ticket))
        {
            let _ = sender.send(if error.is_null() {
                Ok(())
            } else {
                Err(string(error))
            });
        }
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
    pub fn data_dir() -> Option<std::path::PathBuf> {
        // SAFETY: the synchronous adapter getter retains its NSString until the next call.
        let path = unsafe { brigadier_notice_data_dir() };
        (!path.is_null()).then(|| std::path::PathBuf::from(string(path)))
    }
    pub fn ask() {
        // SAFETY: takes no arguments; the completion handler is owned by ObjC.
        unsafe { brigadier_notice_ask() };
    }
    pub fn install(app: AppHandle) {
        let _ = APP.set(app.clone());
        // SAFETY: a static callback, installed during main-thread app setup. ObjC retains its delegate.
        if let Some(state) = app.try_state::<crate::AppState>() {
            let path = state.bridge.data_dir();
            if let Ok(path) = CString::new(path.to_string_lossy().as_bytes()) {
                unsafe { brigadier_notice_init(activated, path.as_ptr()) };
            }
        }
    }
    pub async fn submit(notice: &PendingRunNotification) -> Result<(), String> {
        let values = [
            &notice.notification.id,
            &notice.notification.title,
            &notice.notification.body,
            &notice.conversation_id.0,
            &notice.run_id.0,
        ];
        let strings = values
            .into_iter()
            .map(|value| CString::new(value.as_str()).map_err(|err| err.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let pending = PENDING.get_or_init(|| Mutex::new(HashMap::new()));
        pending
            .lock()
            .map_err(|err| err.to_string())?
            .insert(ticket, sender);
        // SAFETY: all strings are NUL-terminated and live for this call; ObjC copies them
        // before returning. The static callback resolves only its own u64 ticket.
        unsafe {
            brigadier_notice_send(
                strings[0].as_ptr(),
                strings[1].as_ptr(),
                strings[2].as_ptr(),
                strings[3].as_ptr(),
                strings[4].as_ptr(),
                ticket,
                submitted,
            )
        };
        let result = tokio::time::timeout(Duration::from_secs(15), receiver).await;
        pending
            .lock()
            .map_err(|err| err.to_string())?
            .remove(&ticket);
        result
            .map_err(|_| "OS notification submission timed out".to_owned())?
            .map_err(|_| "OS notification completion disappeared".to_owned())?
            .map_err(|err| {
                if err == OFF {
                    NOTIFICATIONS_OFF.to_owned()
                } else {
                    err
                }
            })
    }
}
