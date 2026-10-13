//! The computer-use menu-bar item: which agents are using apps on this Mac, and a way to stop
//! each. Shown while computer use and its "Show in menu bar" setting are on (the webview says
//! which, [`set_computer_menu`]); the agents come from the daemon's computer-use status, which
//! the bridge hands here ([`status_changed`]).

use std::sync::{Mutex, OnceLock};

use brigadier_core::{ComputerSession, ComputerSetup, ComputerUseStatus};
use brigadier_ipc::app::BridgeEvent;
use brigadier_ipc::protocol::Request;
use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::AppState;

/// The item's id.
const TRAY: &str = "computer-use";
/// Menu ids: `<action>:<index into the sessions shown>`.
const OPEN: &str = "cu-open:";
const SHOW: &str = "cu-show:";
const STOP: &str = "cu-stop:";
const SETTINGS: &str = "cu-settings";
const SETUP: &str = "cu-setup";
/// A label longer than this is cut, with an ellipsis.
const LABEL_CHARS: usize = 48;

static APP: OnceLock<AppHandle> = OnceLock::new();
static STATE: Mutex<MenuState> = Mutex::new(MenuState {
    shown: false,
    status: None,
});

struct MenuState {
    shown: bool,
    status: Option<ComputerUseStatus>,
}

fn state() -> std::sync::MutexGuard<'static, MenuState> {
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Remembers the app, for updates that come from the bridge.
pub fn install(app: &AppHandle) {
    let _ = APP.set(app.clone());
}

/// The webview's word on whether the item shows (computer use and "Show in menu bar" are on).
#[tauri::command]
pub fn set_computer_menu(app: AppHandle, shown: bool) {
    state().shown = shown;
    refresh(&app);
}

/// The daemon's computer-use status changed.
pub fn status_changed(status: &ComputerUseStatus) {
    state().status = Some(status.clone());
    if let Some(app) = APP.get() {
        refresh(app);
    }
}

fn refresh(app: &AppHandle) {
    let app = app.clone();
    let main = app.clone();
    let _ = main.run_on_main_thread(move || {
        if let Err(err) = apply(&app) {
            tracing::warn!(%err, "couldn't update the computer-use menu-bar item");
        }
    });
}

/// Shows, updates or removes the item. Main thread.
fn apply(app: &AppHandle) -> tauri::Result<()> {
    let (shown, status) = {
        let state = state();
        (state.shown, state.status.clone())
    };
    if !shown {
        let _ = app.remove_tray_by_id(TRAY);
        return Ok(());
    }
    let sessions = status
        .as_ref()
        .map(|s| s.sessions.clone())
        .unwrap_or_default();
    let menu = menu(app, status.as_ref(), &sessions)?;
    let icon = icon(!sessions.is_empty())?;
    let tooltip = if sessions.is_empty() {
        "Computer use".to_owned()
    } else {
        format!("Computer use: {} in use", plural(sessions.len()))
    };
    if let Some(tray) = app.tray_by_id(TRAY) {
        tray.set_menu(Some(menu))?;
        tray.set_icon(Some(icon))?;
        tray.set_icon_as_template(true)?;
        tray.set_tooltip(Some(tooltip))?;
        return Ok(());
    }
    TrayIconBuilder::with_id(TRAY)
        .icon(icon)
        .icon_as_template(true)
        .tooltip(tooltip)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| chosen(app, event.id().as_ref()))
        .build(app)?;
    Ok(())
}

fn plural(apps: usize) -> String {
    if apps == 1 {
        "1 app".to_owned()
    } else {
        format!("{apps} apps")
    }
}

fn icon(active: bool) -> tauri::Result<Image<'static>> {
    Image::from_bytes(if active {
        include_bytes!("../icons/computer-use-tray-active.png")
    } else {
        include_bytes!("../icons/computer-use-tray.png")
    })
}

fn cut(text: &str) -> String {
    if text.chars().count() > LABEL_CHARS {
        let cut: String = text.chars().take(LABEL_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        text.to_owned()
    }
}

/// A line on setup, when it needs the user or the helper is in trouble.
fn setup_line(setup: &ComputerSetup) -> Option<(&'static str, bool)> {
    match setup {
        ComputerSetup::Missing | ComputerSetup::ConfirmCapture => {
            Some(("Setup isn't finished", true))
        }
        ComputerSetup::Restarting => Some(("The helper isn't responding; restarting it", false)),
        ComputerSetup::Failed { .. } => Some(("Computer use isn't working right now", true)),
        ComputerSetup::Off | ComputerSetup::NotSetUp | ComputerSetup::Ready => None,
    }
}

fn menu(
    app: &AppHandle,
    status: Option<&ComputerUseStatus>,
    sessions: &[ComputerSession],
) -> tauri::Result<Menu<tauri::Wry>> {
    let mut menu = MenuBuilder::new(app);
    if sessions.is_empty() {
        let idle = MenuItemBuilder::with_id("cu-idle", "No agent is using apps right now")
            .enabled(false)
            .build(app)?;
        menu = menu.item(&idle);
    } else {
        let heading = MenuItemBuilder::with_id("cu-heading", "Agents using apps")
            .enabled(false)
            .build(app)?;
        menu = menu.item(&heading);
        for (index, session) in sessions.iter().enumerate() {
            let open = MenuItemBuilder::with_id(format!("{OPEN}{index}"), "Open in Brigadier")
                .enabled(session.conversation_id.is_some())
                .build(app)?;
            let show =
                MenuItemBuilder::with_id(format!("{SHOW}{index}"), format!("Show {}", session.app))
                    .build(app)?;
            let stop = MenuItemBuilder::with_id(
                format!("{STOP}{index}"),
                format!("Stop using {}", session.app),
            )
            .build(app)?;
            let submenu =
                SubmenuBuilder::new(app, cut(&format!("{} · {}", session.label, session.app)))
                    .item(&open)
                    .item(&show)
                    .separator()
                    .item(&stop)
                    .build()?;
            menu = menu.item(&submenu);
        }
    }
    if let Some((line, finish)) = status.and_then(|status| setup_line(&status.setup)) {
        let line = MenuItemBuilder::with_id("cu-setup-line", line)
            .enabled(false)
            .build(app)?;
        menu = menu.separator().item(&line);
        if finish {
            let setup = MenuItemBuilder::with_id(SETUP, "Finish setup…").build(app)?;
            menu = menu.item(&setup);
        }
    }
    let settings = MenuItemBuilder::with_id(SETTINGS, "Computer use settings…").build(app)?;
    menu.separator().item(&settings).build()
}

/// The session the menu showed at `index` (the menu is rebuilt with every status).
fn session(index: &str) -> Option<ComputerSession> {
    let index: usize = index.parse().ok()?;
    state().status.as_ref()?.sessions.get(index).cloned()
}

fn chosen(app: &AppHandle, id: &str) {
    if id == SETTINGS {
        crate::shell::show_main(app);
        emit(app, BridgeEvent::OpenComputerUseSettings);
    } else if id == SETUP {
        crate::computer_setup::open(app, true);
    } else if let Some(session) = id.strip_prefix(OPEN).and_then(session) {
        if let Some(conversation_id) = session.conversation_id {
            crate::shell::show_main(app);
            emit(
                app,
                BridgeEvent::OpenConversation {
                    conversation_id: conversation_id.0,
                },
            );
        }
    } else if let Some(session) = id.strip_prefix(SHOW).and_then(session) {
        show_app(&session);
    } else if let Some(session) = id.strip_prefix(STOP).and_then(session) {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let bridge = state.bridge.clone();
        tauri::async_runtime::spawn(async move {
            let request = Request::StopComputerSession {
                session: session.session.clone(),
                bundle_id: Some(session.bundle_id.clone()).filter(|id| !id.is_empty()),
                app: session.app.clone(),
            };
            if let Err(err) = bridge.request(request).await {
                tracing::warn!(message = %err.message, "couldn't stop the agent using the app");
            }
        });
    }
}

fn emit(app: &AppHandle, event: BridgeEvent) {
    if let Some(state) = app.try_state::<AppState>() {
        state.bridge.emit(event);
    }
}

/// Brings the app the agent drives to the front (the user asked for it).
fn show_app(session: &ComputerSession) {
    let mut open = std::process::Command::new("/usr/bin/open");
    if session.bundle_id.is_empty() {
        open.args(["-a", &session.app]);
    } else {
        open.args(["-b", &session.bundle_id]);
    }
    if let Err(err) = open.spawn().and_then(|mut child| child.wait()) {
        tracing::warn!(%err, app = %session.app, "couldn't show the app");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_shows_only_when_it_needs_the_user_or_the_helper_is_in_trouble() {
        assert_eq!(setup_line(&ComputerSetup::Ready), None);
        assert_eq!(setup_line(&ComputerSetup::NotSetUp), None);
        assert_eq!(setup_line(&ComputerSetup::Off), None);
        assert_eq!(
            setup_line(&ComputerSetup::Missing),
            Some(("Setup isn't finished", true))
        );
        assert_eq!(
            setup_line(&ComputerSetup::Restarting),
            Some(("The helper isn't responding; restarting it", false))
        );
        assert_eq!(
            setup_line(&ComputerSetup::Failed {
                message: "gone".into()
            })
            .map(|(_, finish)| finish),
            Some(true)
        );
    }

    #[test]
    fn long_labels_are_cut_with_an_ellipsis() {
        assert_eq!(cut("Fix the login · TextEdit"), "Fix the login · TextEdit");
        let long = "a".repeat(80);
        let shown = cut(&long);
        assert_eq!(shown.chars().count(), LABEL_CHARS);
        assert!(shown.ends_with('…'));
        assert_eq!(plural(1), "1 app");
        assert_eq!(plural(3), "3 apps");
    }
}
