//! Window, menu-bar item and quit ordering.
//!
//! Closing the window hides it (and the Dock icon on macOS); the app and `brigadierd` keep
//! running in the menu bar. Quit (menu-bar item, Cmd+Q or a system quit) asks the daemon to
//! drain and exit, waits for its acknowledgement, then exits the app.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use brigadier_ipc::app::{BridgeEvent, RunningChat};
#[cfg(target_os = "macos")]
use tauri::Emitter;
use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::AppState;

pub const MAIN_WINDOW: &str = "main";
const TRAY: &str = "brigadier";
/// Menu ids of the "Running" list's items: this, then the conversation's id.
const RUNNING_ITEM: &str = "running:";
/// Longest title the "Running" list shows before cutting it with "…".
const RUNNING_TITLE_CHARS: usize = 48;
/// Longest the app waits for the daemon to acknowledge a quit. Covers the daemon's bounded
/// ending of CLI sessions (exit grace, process-group reap, last events stored) before its
/// store drains; the daemon finishes quitting on its own if this runs out.
const QUIT_TIMEOUT: Duration = Duration::from_secs(10);

static QUITTING: AtomicBool = AtomicBool::new(false);
static IN_MENU_BAR: AtomicBool = AtomicBool::new(false);
/// The code the process exits with once the event loop returns. Tauri's runtime turns
/// `app.exit(code)` into a plain exit, so the event loop itself always reports 0.
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::Acquire)
}

pub fn exit_code() -> i32 {
    EXIT_CODE.load(Ordering::Acquire)
}

/// Whether this process can reach the window server. AppKit aborts the process when it can't
/// (inside a sandbox that denies it, or over SSH without the desktop session), before any of
/// Brigadier's own code runs, so the app checks first.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
pub fn window_server_reachable() -> bool {
    use std::ffi::c_char;
    unsafe extern "C" {
        static bootstrap_port: u32;
        static mach_task_self_: u32;
        fn bootstrap_look_up(bootstrap: u32, name: *const c_char, port: *mut u32) -> i32;
        fn mach_port_deallocate(task: u32, name: u32) -> i32;
    }
    let mut port = 0;
    // SAFETY: looks up a fixed, NUL-terminated service name in this process's bootstrap
    // namespace, writing only the local `port`; the send right it returns is released below.
    let found = unsafe {
        bootstrap_look_up(
            bootstrap_port,
            c"com.apple.windowserver.active".as_ptr(),
            &mut port,
        )
    } == 0;
    if found {
        // SAFETY: `port` is the send right the lookup just gave this task.
        unsafe { mach_port_deallocate(mach_task_self_, port) };
    }
    found
}

/// The menu-bar item's menu: open, the conversations running now ("Running"), quit.
fn tray_menu(app: &AppHandle, running: &[RunningChat]) -> tauri::Result<Menu<tauri::Wry>> {
    let open_item = MenuItemBuilder::with_id("open", "Open Brigadier").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit Brigadier").build(app)?;
    let mut menu = MenuBuilder::new(app).item(&open_item).separator();
    if !running.is_empty() {
        let heading = MenuItemBuilder::with_id("running", "Running")
            .enabled(false)
            .build(app)?;
        menu = menu.item(&heading);
        for chat in running {
            let title = if chat.title.chars().count() > RUNNING_TITLE_CHARS {
                let cut: String = chat.title.chars().take(RUNNING_TITLE_CHARS - 1).collect();
                format!("{}…", cut.trim_end())
            } else {
                chat.title.clone()
            };
            let item =
                MenuItemBuilder::with_id(format!("{RUNNING_ITEM}{}", chat.id), title).build(app)?;
            menu = menu.item(&item);
        }
        menu = menu.separator();
    }
    menu.item(&quit_item).build()
}

/// Lists `running` under "Running" in the menu-bar item's menu.
pub fn set_running(app: &AppHandle, running: &[RunningChat]) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id(TRAY) else {
        return Ok(());
    };
    tray.set_menu(Some(tray_menu(app, running)?))
}

pub fn install_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = tray_menu(app, &[])?;
    TrayIconBuilder::with_id(TRAY)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("Brigadier")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open" => show_main(app),
            "quit" => quit(app, 0),
            id => {
                if let Some(conversation_id) = id.strip_prefix(RUNNING_ITEM) {
                    show_main(app);
                    if let Some(state) = app.try_state::<AppState>() {
                        state.bridge.emit(BridgeEvent::OpenConversation {
                            conversation_id: conversation_id.to_owned(),
                        });
                    }
                }
            }
        })
        .build(app)?;
    IN_MENU_BAR.store(true, Ordering::Release);
    Ok(())
}

/// Menu id of File › Open Folder….
#[cfg(target_os = "macos")]
const OPEN_FOLDER_ITEM: &str = "open-folder";
/// Menu id of Brigadier › Uninstall Brigadier….
#[cfg(target_os = "macos")]
const UNINSTALL_ITEM: &str = "uninstall";
/// Menu id of Brigadier › Settings….
#[cfg(target_os = "macos")]
const SETTINGS_ITEM: &str = "settings";

/// The standard app menu with File › Open Folder… (⌘O) first. Choosing it has the webview ask
/// for folders to add as projects.
#[cfg(target_os = "macos")]
pub fn install_app_menu(app: &AppHandle) -> tauri::Result<()> {
    let menu = Menu::default(app)?;
    let open = MenuItemBuilder::with_id(OPEN_FOLDER_ITEM, "Open Folder…")
        .accelerator("CmdOrCtrl+O")
        .build(app)?;
    let separator = tauri::menu::PredefinedMenuItem::separator(app)?;
    for item in menu.items()? {
        if let Some(file) = item.as_submenu()
            && file.text()? == "File"
        {
            // Cmd+W belongs to the focused browser page or terminal shell, else a session's tab
            // in front. Keep window closing explicit, so a native page cannot make it bypass
            // the pane handler.
            file.remove_at(0)?;
            let close = MenuItemBuilder::with_id("pane:close", "Close tab, page or terminal")
                .accelerator("CmdOrCtrl+W")
                .build(app)?;
            let close_window = MenuItemBuilder::with_id("close-main-window", "Close Window")
                .accelerator("CmdOrCtrl+Shift+W")
                .build(app)?;
            file.insert_items(&[&open, &separator, &close, &close_window], 0)?;
        }
    }
    // The app menu (macOS): Settings… (⌘,) after About, and Uninstall Brigadier… just above
    // Quit.
    #[cfg(target_os = "macos")]
    if let Some(app_menu) = menu
        .items()?
        .first()
        .and_then(|item| item.as_submenu().cloned())
    {
        let settings = MenuItemBuilder::with_id(SETTINGS_ITEM, "Settings…")
            .accelerator("CmdOrCtrl+,")
            .build(app)?;
        let after_about = tauri::menu::PredefinedMenuItem::separator(app)?;
        app_menu.insert_items(&[&after_about, &settings], 1)?;
        let uninstall =
            MenuItemBuilder::with_id(UNINSTALL_ITEM, "Uninstall Brigadier…").build(app)?;
        let separator = tauri::menu::PredefinedMenuItem::separator(app)?;
        let quit_at = app_menu.items()?.len().saturating_sub(1);
        app_menu.insert_items(&[&uninstall, &separator], quit_at)?;
    }
    // View › Toggle Sidebar, Back and Forward at its end; the webview handles them as keys.
    for item in menu.items()? {
        if let Some(view) = item.as_submenu()
            && view.text()? == "View"
        {
            view.append(&tauri::menu::PredefinedMenuItem::separator(app)?)?;
            for (id, title, accelerator) in [
                ("sidebar", "Toggle Sidebar", "CmdOrCtrl+B"),
                ("right-sidebar", "Toggle Right Sidebar", "Alt+CmdOrCtrl+B"),
                ("back", "Back", "CmdOrCtrl+["),
                ("forward", "Forward", "CmdOrCtrl+]"),
            ] {
                view.append(
                    &MenuItemBuilder::with_id(format!("pane:{id}"), title)
                        .accelerator(accelerator)
                        .build(app)?,
                )?;
            }
        }
    }
    let panes = tauri::menu::Submenu::new(app, "Panes", true)?;
    for (id, title, accelerator) in [
        ("terminal", "Toggle bottom terminal", "CmdOrCtrl+J"),
        ("terminal-alternate", "Terminal", "Ctrl+`"),
        ("new", "New tab", "CmdOrCtrl+T"),
        ("new-browser", "New browser tab", "CmdOrCtrl+Shift+B"),
        ("new-side-chat", "New side chat", "Alt+CmdOrCtrl+S"),
        ("new-file", "New file", "Alt+CmdOrCtrl+N"),
        ("save", "Save file", "CmdOrCtrl+S"),
        ("files", "Find file", "CmdOrCtrl+P"),
        ("review", "Review", "Ctrl+Shift+G"),
        ("cycle-next", "Next session tab", "Ctrl+Tab"),
        ("cycle-previous", "Previous session tab", "Ctrl+Shift+Tab"),
        (
            "reopen",
            "Reopen closed tab, page or terminal",
            "CmdOrCtrl+Shift+T",
        ),
        ("address", "Focus browser address", "CmdOrCtrl+L"),
        (
            "previous",
            "Previous tab, page or terminal",
            "CmdOrCtrl+Shift+[",
        ),
        ("next", "Next tab, page or terminal", "CmdOrCtrl+Shift+]"),
    ] {
        panes.append(
            &MenuItemBuilder::with_id(format!("pane:{id}"), title)
                .accelerator(accelerator)
                .build(app)?,
        )?;
    }
    for number in 1..=9 {
        panes.append(
            &MenuItemBuilder::with_id(
                format!("pane:tab-{number}"),
                if number == 1 {
                    "Show chat".to_owned()
                } else {
                    format!("Show tab {number}")
                },
            )
            .accelerator(format!("CmdOrCtrl+{number}"))
            .build(app)?,
        )?;
    }
    menu.append(&panes)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if let Some(shortcut) = event.id().as_ref().strip_prefix("pane:") {
            let _ = app.emit_to(MAIN_WINDOW, "pane-shortcut", shortcut);
            return;
        }
        if event.id().as_ref() == "close-main-window" {
            if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                let _ = window.close();
            }
            return;
        }
        let bridge_event = match event.id().as_ref() {
            OPEN_FOLDER_ITEM => BridgeEvent::OpenFolderMenu,
            UNINSTALL_ITEM => BridgeEvent::UninstallMenu,
            SETTINGS_ITEM => BridgeEvent::SettingsMenu,
            _ => return,
        };
        show_main(app);
        if let Some(state) = app.try_state::<AppState>() {
            state.bridge.emit(bridge_event);
        }
    });
    Ok(())
}

/// Closing the window hides it when the menu-bar item can bring it back; without one (a
/// desktop with no tray host) closing quits.
pub fn close_main(app: &AppHandle) {
    if IN_MENU_BAR.load(Ordering::Acquire) {
        hide_main(app);
    } else {
        quit(app, 0);
    }
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(MAIN_WINDOW)
}

pub fn show_main(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = main_window(app) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    notify_visibility(app, true);
}

pub fn hide_main(app: &AppHandle) {
    if let Some(window) = main_window(app) {
        let _ = window.hide();
    }
    // A menu-bar app has no Dock icon while its window is closed.
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    notify_visibility(app, false);
}

fn notify_visibility(app: &AppHandle, visible: bool) {
    if let Some(state) = app.try_state::<AppState>() {
        state.bridge.emit(BridgeEvent::WindowVisibility { visible });
    }
}

/// Orderly quit: the daemon drains and acknowledges, then the app exits with `code`.
pub fn quit(app: &AppHandle, code: i32) {
    if QUITTING.swap(true, Ordering::AcqRel) {
        return;
    }
    EXIT_CODE.store(code, Ordering::Release);
    if let Some(window) = main_window(app) {
        let _ = window.hide();
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        stop_daemon(&app).await;
        app.exit(code);
    });
}

/// The process is already exiting without an orderly quit: on macOS, `terminate:` (the app
/// menu's Quit, the Dock, AppleScript, logout) skips `ExitRequested` and only surfaces as
/// `RunEvent::Exit`. Runs the same drain-and-acknowledge before the process goes away.
pub fn quit_on_exit(app: &AppHandle) {
    if QUITTING.swap(true, Ordering::AcqRel) {
        return;
    }
    tauri::async_runtime::block_on(stop_daemon(app));
}

async fn stop_daemon(app: &AppHandle) {
    if crate::overnight_notifications::is_host() {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let bridge = state.bridge.clone();
    // An overnight run owns the daemon until it ends: the app goes, the run goes on
    // (PLAN.md §10.10).
    if let Ok(Ok(brigadier_ipc::protocol::Response::GetDaemonActivity { activity })) =
        tokio::time::timeout(
            QUIT_TIMEOUT,
            bridge.request(brigadier_ipc::protocol::Request::GetDaemonActivity),
        )
        .await
        && activity.overnight
    {
        tracing::info!("an overnight run is under way; brigadierd keeps running");
        return;
    }
    if bridge.shutdown_daemon(QUIT_TIMEOUT).await {
        tracing::info!("brigadierd drained and acknowledged the quit");
    } else {
        tracing::warn!("brigadierd did not acknowledge the quit");
    }
}
