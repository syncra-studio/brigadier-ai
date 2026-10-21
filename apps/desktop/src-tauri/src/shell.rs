//! Window, menu-bar item and quit ordering.
//!
//! Closing the window hides it (and the Dock icon on macOS); the app and `brigadierd` keep
//! running in the menu bar. Quit (menu-bar item, Cmd+Q or a system quit) asks the daemon to
//! drain and exit, waits for its acknowledgement, then exits the app.
//!
//! Before that, an orderly quit (Brigadier › Quit and Cmd+Q, the menu-bar item's Quit, closing
//! the window where there is no menu-bar item, the app's own Quit) has the window save its
//! pages' pending edits: it hears `flush-pages` with a token, saves, and answers
//! `pages_flushed` with the token, or the quit goes on after [`FLUSH_TIMEOUT`]. A quit that
//! can't wait for the window gets no flush: on macOS a system quit (the Dock's Quit,
//! AppleScript, logout, restart) arrives only as the process exits, with the event loop that
//! would carry the window's answer no longer turning; and a crash, a force quit or a kill
//! signal end the app at once. The window keeps its own copy of unsaved edits for those.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brigadier_ipc::app::{BridgeEvent, RunningChat};
use tauri::Emitter;
use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, WebviewWindow};
use tokio::sync::oneshot;

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
/// Longest a quit waits for the window to save its pages' pending edits.
const FLUSH_TIMEOUT: Duration = Duration::from_millis(1500);
/// The window's event asking it to save its pages' pending edits now.
const FLUSH_PAGES: &str = "flush-pages";
/// The flush waiting for the window's answer: its token and who hears the answer.
static FLUSH: Mutex<Option<(u64, oneshot::Sender<()>)>> = Mutex::new(None);
/// A window close and a quit must each finish their save request before the next replaces it.
static FLUSH_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static NEXT_FLUSH: AtomicU64 = AtomicU64::new(1);

static QUITTING: AtomicBool = AtomicBool::new(false);
static IN_MENU_BAR: AtomicBool = AtomicBool::new(false);
/// The code the process exits with once the event loop returns. Tauri's runtime turns
/// `app.exit(code)` into a plain exit, so the event loop itself always reports 0.
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);
/// The quit stopped the daemon (it acknowledged), so the app's files can be replaced.
static DAEMON_STOPPED: AtomicBool = AtomicBool::new(false);

/// Until when (ms since the Unix epoch) Settings records a key press. Meanwhile the menu's own
/// items (Settings…, Open Folder…, Close Window) reach the page as key presses instead of acting.
static KEY_CAPTURE_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
/// Longest a recording holds those items, should the page never say it ended.
const KEY_CAPTURE_LIMIT: Duration = Duration::from_secs(60);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Settings starts (`true`) or ends recording a key press.
pub fn set_key_capture(on: bool) {
    let until = if on {
        now_ms().saturating_add(u64::try_from(KEY_CAPTURE_LIMIT.as_millis()).unwrap_or(u64::MAX))
    } else {
        0
    };
    KEY_CAPTURE_UNTIL_MS.store(until, Ordering::Release);
}

#[cfg(target_os = "macos")]
fn capturing_keys() -> bool {
    now_ms() < KEY_CAPTURE_UNTIL_MS.load(Ordering::Acquire)
}

pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::Acquire)
}

pub fn exit_code() -> i32 {
    EXIT_CODE.load(Ordering::Acquire)
}

pub fn daemon_stopped() -> bool {
    DAEMON_STOPPED.load(Ordering::Acquire)
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
/// Menu id of Help › Send Feedback….
#[cfg(target_os = "macos")]
const FEEDBACK_ITEM: &str = "send-feedback";
/// Menu id of Brigadier › Quit.
#[cfg(target_os = "macos")]
const QUIT_ITEM: &str = "app-quit";

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
        // The standard Quit is the system's `terminate:`, which the app hears only as it exits,
        // too late for the window to save its pages: ours quits the orderly way.
        let quit_at = app_menu.items()?.len().saturating_sub(1);
        app_menu.remove_at(quit_at)?;
        let title = format!("Quit {}", app.package_info().name);
        let quit = MenuItemBuilder::with_id(QUIT_ITEM, title)
            .accelerator("CmdOrCtrl+Q")
            .build(app)?;
        app_menu.append(&quit)?;
    }
    // View › Toggle Sidebar, the right sidebar and its views, Back and Forward at its end; the
    // webview handles them as keys.
    for item in menu.items()? {
        if let Some(view) = item.as_submenu()
            && view.text()? == "View"
        {
            view.append(&tauri::menu::PredefinedMenuItem::separator(app)?)?;
            for (id, title, accelerator) in [
                ("sidebar", "Toggle Sidebar", "CmdOrCtrl+B"),
                ("right-sidebar", "Toggle Right Sidebar", "Alt+CmdOrCtrl+B"),
                ("show-files", "Files", "CmdOrCtrl+Shift+E"),
                ("show-search", "Search", "CmdOrCtrl+Shift+F"),
                ("show-source", "Source", "Ctrl+Shift+G"),
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
        ("new-side-chat", "New side chat", "Alt+CmdOrCtrl+S"),
        ("new-page", "New page", "Alt+CmdOrCtrl+N"),
        ("save", "Save file", "CmdOrCtrl+S"),
        ("save-as", "Save file as…", "CmdOrCtrl+Shift+S"),
        ("files", "Find file", "CmdOrCtrl+P"),
        ("show-pages", "Show pages", "Alt+CmdOrCtrl+E"),
        ("cycle-next", "Next session tab", "Ctrl+Tab"),
        ("cycle-previous", "Previous session tab", "Ctrl+Shift+Tab"),
        (
            "reopen",
            "Reopen closed tab, page or terminal",
            "CmdOrCtrl+Shift+T",
        ),
        (
            "previous",
            "Previous tab, page or terminal",
            "CmdOrCtrl+Shift+[",
        ),
        ("next", "Next tab, page or terminal", "CmdOrCtrl+Shift+]"),
        ("previous-tab", "Previous tab", "Alt+CmdOrCtrl+Left"),
        ("next-tab", "Next tab", "Alt+CmdOrCtrl+Right"),
        ("close-others", "Close other tabs", "Alt+CmdOrCtrl+W"),
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
    menu.append(&browser_submenu(app)?)?;
    // Help › Send Feedback…, in the standard Help menu (last), or a Help menu of its own.
    let feedback = MenuItemBuilder::with_id(FEEDBACK_ITEM, "Send Feedback…").build(app)?;
    let mut help = None;
    for item in menu.items()? {
        if let Some(submenu) = item.as_submenu()
            && submenu.text()? == "Help"
        {
            help = Some(submenu.clone());
        }
    }
    match help {
        Some(help) => help.append(&feedback)?,
        None => {
            let help = tauri::menu::Submenu::new(app, "Help", true)?;
            help.append(&feedback)?;
            menu.append(&help)?;
        }
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if let Some(shortcut) = event.id().as_ref().strip_prefix("pane:") {
            let _ = app.emit_to(MAIN_WINDOW, "pane-shortcut", shortcut);
            return;
        }
        // While Settings records a key press, the menu's own items are key presses too.
        if capturing_keys() {
            let key = match event.id().as_ref() {
                "close-main-window" => Some("close-window"),
                SETTINGS_ITEM => Some("settings"),
                OPEN_FOLDER_ITEM => Some("open-folder"),
                _ => None,
            };
            if let Some(key) = key {
                let _ = app.emit_to(MAIN_WINDOW, "pane-shortcut", key);
                return;
            }
        }
        if event.id().as_ref() == QUIT_ITEM {
            quit(app, 0);
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
            FEEDBACK_ITEM => BridgeEvent::FeedbackMenu,
            _ => return,
        };
        show_main(app);
        if let Some(state) = app.try_state::<AppState>() {
            state.bridge.emit(bridge_event);
        }
    });
    Ok(())
}

/// The Browser menu's items, in groups: what opens a page, then what acts on the page in front.
/// Each reaches the page's tab as its key press, also while the page has the keyboard; with no
/// web page in front a key does its usual job.
#[cfg(target_os = "macos")]
const BROWSER_ITEMS: [&[(&str, &str, &str)]; 4] = [
    &[
        ("new-browser", "New browser tab", "CmdOrCtrl+Shift+B"),
        ("address", "Focus browser address", "CmdOrCtrl+L"),
    ],
    &[
        ("page-back", "Page back", "CmdOrCtrl+Left"),
        ("page-forward", "Page forward", "CmdOrCtrl+Right"),
        ("reload", "Reload page", "CmdOrCtrl+R"),
        (
            "force-reload",
            "Reload page without the cache",
            "CmdOrCtrl+Shift+R",
        ),
    ],
    &[
        ("find", "Find…", "CmdOrCtrl+F"),
        ("find-next", "Find next", "CmdOrCtrl+G"),
        ("find-previous", "Find previous", "CmdOrCtrl+Shift+G"),
    ],
    &[
        ("zoom-in", "Zoom in", "CmdOrCtrl+="),
        ("zoom-out", "Zoom out", "CmdOrCtrl+-"),
        ("zoom-reset", "Actual size", "CmdOrCtrl+0"),
    ],
];

/// The Browser menu's title, which finds it again.
#[cfg(target_os = "macos")]
const BROWSER_MENU: &str = "Browser";

#[cfg(target_os = "macos")]
fn browser_submenu(app: &AppHandle) -> tauri::Result<tauri::menu::Submenu<tauri::Wry>> {
    let browser = tauri::menu::Submenu::new(app, BROWSER_MENU, true)?;
    for (at, group) in BROWSER_ITEMS.iter().enumerate() {
        if at > 0 {
            browser.append(&tauri::menu::PredefinedMenuItem::separator(app)?)?;
        }
        for (id, title, accelerator) in group.iter() {
            browser.append(
                &MenuItemBuilder::with_id(format!("pane:{id}"), *title)
                    .accelerator(*accelerator)
                    .build(app)?,
            )?;
        }
    }
    Ok(browser)
}

/// Takes the Browser menu out of the menu bar (Settings › Browser off), or puts it back.
#[cfg(target_os = "macos")]
pub fn browser_menu(app: &AppHandle, on: bool) -> tauri::Result<()> {
    let Some(menu) = app.menu() else {
        return Ok(());
    };
    let shown = menu.items()?.into_iter().find_map(|item| {
        item.as_submenu()
            .filter(|submenu| submenu.text().is_ok_and(|text| text == BROWSER_MENU))
            .cloned()
    });
    match (on, shown) {
        (false, Some(browser)) => {
            menu.remove(&browser)?;
        }
        (true, None) => menu.append(&browser_submenu(app)?)?,
        _ => {}
    }
    Ok(())
}

/// Closing the window saves its pages before hiding it when the menu-bar item can bring it
/// back; without one (a desktop with no tray host) closing quits.
pub fn close_main(app: &AppHandle) {
    if IN_MENU_BAR.load(Ordering::Acquire) {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            flush_pages(&app).await;
            hide_main(&app);
        });
    } else {
        quit(app, 0);
    }
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(MAIN_WINDOW)
}

/// A build a session launched as an app preview (#142): it never takes the focus itself, at
/// launch, for its intro, a folder it is handed or a second launch, until the user comes to it.
static BACKGROUND: AtomicBool = AtomicBool::new(false);

/// This build was launched in the background.
pub fn launch_in_background() {
    BACKGROUND.store(true, Ordering::SeqCst);
}

/// The user came to the app (its Dock icon): from now on it comes forward as usual.
#[cfg(target_os = "macos")]
pub fn user_came_to_it() {
    BACKGROUND.store(false, Ordering::SeqCst);
}

pub fn show_main(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
    if let Some(window) = main_window(app) {
        let _ = window.unminimize();
        let _ = window.show();
        if !BACKGROUND.load(Ordering::SeqCst) {
            let _ = window.set_focus();
        }
    }
    notify_visibility(app, true);
}

pub fn hide_main(app: &AppHandle) {
    // The first launch's intro doesn't outlive its window being seen.
    crate::intro::interrupt(app);
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

/// Orderly quit: the window saves its pages, the daemon drains and acknowledges, then the app
/// exits with `code`.
pub fn quit(app: &AppHandle, code: i32) {
    if QUITTING.swap(true, Ordering::AcqRel) {
        return;
    }
    EXIT_CODE.store(code, Ordering::Release);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // While the window is still shown and running, and the daemon still takes saves.
        flush_pages(&app).await;
        if let Some(window) = main_window(&app) {
            let _ = window.hide();
        }
        stop_daemon(&app).await;
        app.exit(code);
    });
}

/// Asks the window to save its pages' pending edits and waits for its answer, at most
/// [`FLUSH_TIMEOUT`].
async fn flush_pages(app: &AppHandle) {
    if crate::overnight_notifications::is_host() || main_window(app).is_none() {
        return;
    }
    let _held = FLUSH_GATE.lock().await;
    let token = NEXT_FLUSH.fetch_add(1, Ordering::Relaxed);
    let (answered, answer) = oneshot::channel();
    *FLUSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((token, answered));
    if let Err(err) = app.emit_to(MAIN_WINDOW, FLUSH_PAGES, token) {
        tracing::warn!(error = %err, "could not ask the window to save its pages");
        return;
    }
    match tokio::time::timeout(FLUSH_TIMEOUT, answer).await {
        Ok(Ok(())) => tracing::info!("the window saved its pages"),
        _ => tracing::warn!("the window did not say its pages were saved; quitting anyway"),
    }
}

/// The window's answer to `flush-pages`: its pages' pending edits are saved (or failed).
pub fn pages_flushed(token: u64) {
    let mut flush = FLUSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if flush.as_ref().is_some_and(|(waiting, _)| *waiting == token)
        && let Some((_, answered)) = flush.take()
    {
        let _ = answered.send(());
    }
}

/// The process is already exiting without an orderly quit: on macOS, `terminate:` (the Dock,
/// AppleScript, logout) skips `ExitRequested` and only surfaces as `RunEvent::Exit`. Runs the
/// same drain-and-acknowledge before the process goes away. The window's pages are not saved
/// first: this blocks the event loop that would carry its answer.
pub fn quit_on_exit(app: &AppHandle) {
    let app = app.clone();
    on_exit(move || {
        let stopped = wait_on_runtime(
            tauri::async_runtime::handle().inner(),
            async move { stop_daemon(&app).await },
            EXIT_WAIT,
        );
        if stopped.is_none() {
            tracing::warn!("brigadierd did not answer the quit in time; exiting anyway");
        }
    });
}

/// The app's last event, on every quit: stops the daemon unless an orderly quit already has,
/// then installs a waiting update. After a quit Apple event AppKit ends the process as soon as
/// this returns (`terminate:` exits after `applicationWillTerminate:`), so `run_return` never
/// returns and the update can't wait for it (#349).
fn on_exit(stop_daemon: impl FnOnce()) {
    on_exit_with(&QUITTING, stop_daemon, daemon_stopped);
}

/// [`on_exit`] with the flag that marks the quit and how it reads whether the daemon stopped.
/// Tests pass their own: a set `QUITTING` silences the bridge's notices for the whole process.
fn on_exit_with(
    quitting: &AtomicBool,
    stop_daemon: impl FnOnce(),
    daemon_stopped: impl Fn() -> bool,
) {
    if !quitting.swap(true, Ordering::AcqRel) {
        stop_daemon();
    }
    crate::updates::finish(daemon_stopped());
}

/// Longest the main thread waits for the daemon when the process is already exiting: the two
/// requests [`stop_daemon`] makes, each bounded by [`QUIT_TIMEOUT`].
const EXIT_WAIT: Duration = Duration::from_secs(2 * QUIT_TIMEOUT.as_secs());

/// Runs `task` on `runtime` and waits for it on this thread, at most `limit` by the clock.
/// The wait doesn't drive the runtime: on the main thread, a runtime thread answering the
/// webview or reading a window waits on this one, and with every runtime thread stuck so the
/// runtime's own timers stop; a plain wait still ends.
fn wait_on_runtime<T: Send + 'static>(
    runtime: &tokio::runtime::Handle,
    task: impl Future<Output = T> + Send + 'static,
    limit: Duration,
) -> Option<T> {
    let (done, finished) = std::sync::mpsc::sync_channel(1);
    runtime.spawn(async move {
        let _ = done.send(task.await);
    });
    finished.recv_timeout(limit).ok()
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
        DAEMON_STOPPED.store(true, Ordering::Release);
    } else {
        tracing::warn!("brigadierd did not acknowledge the quit");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::{AtomicBool, Ordering, on_exit_with, wait_on_runtime};

    /// A quit Apple event (the Dock, logout, AppleScript) ends the process right after the
    /// app's last event, so `run_return` never returns: the waiting update goes in at that
    /// event, once, and the call after `run_return` (Cmd+Q) finds none left (#349).
    #[test]
    fn a_quit_that_never_returns_from_the_run_loop_installs_the_waiting_update() {
        let installs = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        crate::updates::wait_counting("0.1.0-nightly.202610190100", installs.clone());
        let (quitting, stopped) = (AtomicBool::new(false), AtomicBool::new(false));
        let daemon_stopped = || stopped.load(Ordering::Acquire);
        on_exit_with(
            &quitting,
            || stopped.store(true, Ordering::Release),
            daemon_stopped,
        );
        assert_eq!(
            installs.load(Ordering::Acquire),
            1,
            "installed at the last event"
        );
        assert!(!crate::updates::finish(daemon_stopped()));
        assert_eq!(installs.load(Ordering::Acquire), 1, "installed once");
    }

    /// The exit path's wait ends by the clock when every runtime thread is stuck waiting on the
    /// waiting thread, as a webview response or a window getter is during a system quit.
    #[test]
    fn the_exit_wait_ends_when_every_runtime_thread_waits_on_it() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        // The one runtime thread waits for this thread, which is busy waiting below.
        let (release, stuck) = mpsc::channel::<()>();
        runtime.spawn(async move {
            let _ = stuck.recv();
        });
        let (outcome, waited) = mpsc::channel();
        let handle = runtime.handle().clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let stopped = wait_on_runtime(
                &handle,
                async {
                    tokio::time::timeout(Duration::from_millis(50), std::future::pending::<()>())
                        .await
                },
                Duration::from_millis(300),
            );
            let _ = outcome.send((stopped.is_some(), started.elapsed()));
        });
        let (stopped, took) = waited
            .recv_timeout(Duration::from_secs(5))
            .expect("the exit wait never ended");
        assert!(!stopped);
        assert!(took < Duration::from_secs(2), "took {took:?}");
        let _ = release.send(());
    }

    /// The main window's page runs while the window is unseen (launched in the background,
    /// behind other windows): WebKit would otherwise suspend it, and onboarding and the folder
    /// trust prompt would wait for the window to be brought forward (#134).
    #[test]
    fn the_main_window_keeps_running_while_unseen() {
        for config in [
            include_str!("../tauri.conf.json"),
            include_str!("../tauri.macos.conf.json"),
        ] {
            let config: serde_json::Value = serde_json::from_str(config).unwrap();
            let windows = config.pointer("/app/windows").unwrap().as_array().unwrap();
            let main = windows
                .iter()
                .find(|window| window["label"] == super::MAIN_WINDOW)
                .unwrap();
            assert_eq!(main["backgroundThrottling"], "disabled");
        }
    }

    #[test]
    fn the_exit_wait_answers_what_the_task_returns() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("runtime");
        let answer = wait_on_runtime(runtime.handle(), async { 7 }, Duration::from_secs(5));
        assert_eq!(answer, Some(7));
    }
}
