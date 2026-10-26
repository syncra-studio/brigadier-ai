//! Brigadier desktop shell.
//!
//! The React UI runs in the system webview. This process is its only link to `brigadierd`:
//! it launches the daemon detached, keeps an authenticated connection to it, and bridges
//! requests and the live event feed to the webview. It also owns the window, the menu-bar item
//! and the quit order.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bridge;
mod browser;
mod browser_agent;
#[cfg(any(target_os = "macos", test))]
mod browser_grants;
mod browser_import;
mod browser_store;
mod computer_menu;
mod computer_setup;
mod documents;
mod downloads;
mod drops;
mod intro;
// WebKit's narrow unsafe boundary (the workspace denies it): see its header and the README.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod browser_ui;
mod launcher;
mod login_item;
mod notifications;
mod reports;
mod shell;
mod simulator;
mod smoke;
mod updates;

use std::sync::Arc;
use std::sync::Mutex;

use brigadier_core::storage::UninstallApp;
use brigadier_core::{ConventionsExport, ProjectId};
use brigadier_ipc::app::{AppInfo, BridgeEvent, RunningChat, SmokeReport, UiMeasurements};
use brigadier_ipc::protocol::{IpcError, Request, Response};
use brigadier_sandbox::{Platform, PlatformOptions};
use tauri::ipc::Channel;
use tauri::{Manager, RunEvent, State, WindowEvent};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use crate::bridge::{Bridge, Notify};
use crate::launcher::Launcher;

/// Environment variable carrying the timing tolerance (shared with the daemon).
const TOLERANCE_ENV: &str = "BRIGADIER_BUDGET_TOLERANCE";
/// The installed app's identity, and the one debug builds take instead (`tauri.dev.conf.json`).
const RELEASE_IDENTIFIER: &str = "ai.brigadier.app";
const DEBUG_IDENTIFIER: &str = "ai.brigadier.dev";
/// The exit code when there's no window server to draw on (1 is a startup error, 2 a smoke
/// check that failed).
#[cfg(target_os = "macos")]
const NO_WINDOW_SERVER: i32 = 3;

pub struct AppState {
    pub bridge: Bridge,
    info: AppInfo,
    cold_start_ms: Mutex<Option<f64>>,
}

/// Folders opened with the app that the webview hasn't taken yet. Not in [`AppState`]: a
/// launch by opening a folder delivers it before the app is set up.
static OPENED_FOLDERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[tauri::command]
fn app_info(state: State<'_, AppState>) -> AppInfo {
    state.info.clone()
}

/// This computer's system and its processor, for feedback ("macOS 27.0 Tahoe (aarch64)").
#[tauri::command]
fn system_name() -> String {
    let system =
        sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_owned());
    format!("{system} ({})", std::env::consts::ARCH)
}

/// The running app, for Uninstall Brigadier…: its bundle identifier, the app bundle it runs
/// from (macOS) and its pid.
#[tauri::command]
fn uninstall_app(app: tauri::AppHandle) -> UninstallApp {
    let bundle_path = if cfg!(target_os = "macos") {
        std::env::current_exe().ok().and_then(|exe| {
            exe.ancestors()
                .find(|dir| dir.extension().is_some_and(|ext| ext == "app"))
                .map(|dir| dir.display().to_string())
        })
    } else {
        None
    };
    UninstallApp {
        identifier: app.config().identifier.clone(),
        bundle_path,
        pid: std::process::id(),
    }
}

/// Quits the app the orderly way (the daemon drains and quits first).
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    shell::quit(&app, 0);
}

/// The window saved its pages' pending edits, as a quit's `flush-pages` asked with `token`.
#[tauri::command]
fn pages_flushed(token: u64) {
    shell::pages_flushed(token);
}

#[tauri::command]
async fn ipc_request(state: State<'_, AppState>, request: Request) -> Result<Response, IpcError> {
    state.bridge.request(request).await
}

/// Whether Brigadier may show notifications, without asking (a run's card says when not).
#[tauri::command]
async fn notification_permission() -> notifications::Permission {
    notifications::permission().await
}

/// Asks the system to let Brigadier show notifications (Settings › General), and answers what
/// the user chose once they did (still not asked after a minute without an answer).
#[tauri::command]
async fn ask_notification_permission() -> notifications::Permission {
    notifications::ask_and_wait().await
}

/// Opens the system's notification settings at Brigadier.
#[tauri::command]
fn open_notification_settings(app: tauri::AppHandle) -> Result<(), String> {
    notifications::open_settings(&app)
}

#[tauri::command]
fn ipc_subscribe(state: State<'_, AppState>, channel: Channel<BridgeEvent>) {
    state.bridge.attach_ui(channel);
}

/// Asks for a folder with the system picker; `None` when the user cancels. The dialog plugin
/// is used from here only: the webview gets no dialog permissions of its own.
#[tauri::command]
async fn pick_folder(app: tauri::AppHandle, starting: Option<String>) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Choose a folder");
    if let Some(window) = app.get_webview_window(shell::MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    if let Some(starting) = starting.filter(|dir| std::path::Path::new(dir).is_dir()) {
        dialog = dialog.set_directory(starting);
    }
    dialog.pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    let folder = rx.await.ok().flatten()?.into_path().ok()?;
    Some(folder.display().to_string())
}

/// Readies the system folder picker without showing anything, so the first `pick_folder` opens
/// at once: on macOS the first open panel of a run loads its frameworks (about half a second).
/// Only the first call does anything.
#[tauri::command]
fn warm_folder_picker(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        static WARMED: std::sync::Once = std::sync::Once::new();
        WARMED.call_once(|| {
            let _ = app.run_on_main_thread(|| {
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    drop(objc2_app_kit::NSOpenPanel::openPanel(mtm));
                }
            });
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

/// Asks for one file with the system picker, titled `title` and opening in `starting` when that
/// folder exists; `None` when the user cancels.
#[tauri::command]
async fn pick_file(
    app: tauri::AppHandle,
    title: String,
    starting: Option<String>,
) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title(title);
    if let Some(window) = app.get_webview_window(shell::MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    if let Some(starting) = starting.filter(|dir| std::path::Path::new(dir).is_dir()) {
        dialog = dialog.set_directory(starting);
    }
    dialog.pick_file(move |file| {
        let _ = tx.send(file);
    });
    let file = rx.await.ok().flatten()?.into_path().ok()?;
    Some(file.display().to_string())
}

/// File › Open Folder…: asks for folders to add as projects; empty when the user cancels.
#[tauri::command]
async fn pick_folders(app: tauri::AppHandle) -> Vec<String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app.dialog().file().set_title("Open Folder");
    if let Some(window) = app.get_webview_window(shell::MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_folders(move |folders| {
        let _ = tx.send(folders);
    });
    rx.await
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|folder| folder.into_path().ok())
        .map(|folder| folder.display().to_string())
        .collect()
}

/// The folders opened with the app since the last call (see [`open_folders`]).
#[tauri::command]
fn take_opened_folders() -> Vec<String> {
    std::mem::take(&mut *OPENED_FOLDERS.lock().expect("opened folders lock"))
}

/// Folders opened with the app (Finder, the Dock, `open -a`, a command-line argument): the
/// webview takes them and adds them as projects. Anything but an existing folder is ignored.
fn open_folders(app: &tauri::AppHandle, paths: impl IntoIterator<Item = std::path::PathBuf>) {
    let folders: Vec<String> = paths
        .into_iter()
        .filter(|path| path.is_dir())
        .map(|path| path.display().to_string())
        .collect();
    if folders.is_empty() {
        return;
    }
    OPENED_FOLDERS
        .lock()
        .expect("opened folders lock")
        .extend(folders);
    // Before setup, the webview takes them once it connects.
    if let Some(state) = app.try_state::<AppState>() {
        shell::show_main(app);
        state.bridge.emit(BridgeEvent::FoldersOpened);
    }
}

/// The app's options whose value is the next argument: that value is never a folder to open
/// (a relaunch with `--brigadier-data-dir` offered its own data folder as a project).
const VALUE_OPTIONS: [&str; 1] = ["--brigadier-data-dir"];

/// The folders among a launch's arguments, relative ones resolved against `cwd`.
fn folder_args(
    args: impl IntoIterator<Item = String>,
    cwd: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    let mut args = args.into_iter();
    let mut folders = Vec::new();
    while let Some(arg) = args.next() {
        if VALUE_OPTIONS.contains(&arg.as_str()) {
            args.next();
        } else if !arg.starts_with('-') {
            let path = cwd.join(arg);
            if path.is_dir() {
                folders.push(path);
            }
        }
    }
    folders
}

/// Saves an artifact where the user picks in the system save dialog, offering `file_name`.
/// `false` when they cancel. Scripts use the `saveArtifact` request with a path instead.
#[tauri::command]
async fn save_artifact(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    file_name: String,
) -> Result<bool, IpcError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Save to…")
        .set_file_name(&file_name);
    if let Some(window) = app.get_webview_window(shell::MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    dialog.save_file(move |path| {
        let _ = tx.send(path);
    });
    let Some(path) = rx
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(false);
    };
    state
        .bridge
        .request(Request::SaveArtifact {
            id,
            path: path.display().to_string(),
        })
        .await?;
    Ok(true)
}

/// Exports a project's conventions to the AGENTS.md the user picks in the system save dialog,
/// starting in `directory` (the project's repository). `None` when they cancel. Scripts use the
/// `exportConventions` request with a path instead.
#[tauri::command]
async fn export_conventions(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    project_id: ProjectId,
    directory: Option<String>,
) -> Result<Option<ConventionsExport>, IpcError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut dialog = app
        .dialog()
        .file()
        .set_title("Export conventions")
        .set_file_name("AGENTS.md");
    if let Some(window) = app.get_webview_window(shell::MAIN_WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    if let Some(directory) = directory.filter(|dir| std::path::Path::new(dir).is_dir()) {
        dialog = dialog.set_directory(directory);
    }
    dialog.save_file(move |path| {
        let _ = tx.send(path);
    });
    let Some(path) = rx
        .await
        .ok()
        .flatten()
        .and_then(|path| path.into_path().ok())
    else {
        return Ok(None);
    };
    let Response::ExportConventions { export } = state
        .bridge
        .request(Request::ExportConventions {
            project_id,
            path: path.display().to_string(),
        })
        .await?
    else {
        return Err(IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Internal,
            message: "unexpected response".into(),
        });
    };
    Ok(Some(export))
}

/// Opens an artifact with the system's default app for its type (a copy of it, named
/// `file_name`).
#[tauri::command]
async fn open_artifact(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    id: String,
    file_name: String,
) -> Result<(), IpcError> {
    let internal = |message: String| IpcError {
        code: brigadier_ipc::protocol::ErrorCode::Internal,
        message,
    };
    let Response::OpenArtifact { path } = state
        .bridge
        .request(Request::OpenArtifact { id, file_name })
        .await?
    else {
        return Err(internal("unexpected response".into()));
    };
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|err| internal(format!("could not open it: {err}")))
}

/// Opens a folder (a session's working directory) in the system file manager, or with the
/// app named `with` ("Terminal" on macOS).
#[tauri::command]
fn open_folder(app: tauri::AppHandle, path: String, with: Option<String>) -> Result<(), IpcError> {
    app.opener()
        .open_path(path, with.as_deref())
        .map_err(|err| IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Internal,
            message: format!("could not open it: {err}"),
        })
}

/// Opens a web link in the user's browser, or an email link as a new message in their mail app
/// (for them to send or not). Only http, https and mailto: the thread's links come from models
/// and must not launch other handlers.
#[tauri::command]
fn open_url(app: tauri::AppHandle, url: String) -> Result<(), IpcError> {
    let invalid = |message: String| IpcError {
        code: brigadier_ipc::protocol::ErrorCode::Invalid,
        message,
    };
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    if !matches!(scheme.as_deref(), Some("http" | "https" | "mailto")) {
        return Err(invalid(format!("only web and email links open: {url}")));
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|err| IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Internal,
            message: format!("could not open it: {err}"),
        })
}

/// Settings starts or ends recording a key press (search by keystrokes): meanwhile the menu's
/// own items reach the page as key presses.
#[tauri::command]
fn set_key_capture(on: bool) {
    shell::set_key_capture(on);
}

/// The conversations running now, which the menu-bar item lists under "Running".
#[tauri::command]
fn set_running_chats(app: tauri::AppHandle, chats: Vec<RunningChat>) {
    if let Err(err) = shell::set_running(&app, &chats) {
        tracing::warn!(error = %err, "could not update the menu-bar item");
    }
}

/// Opens a rendered PDF (a PDF preview's file) in the system's PDF viewer. Only an existing
/// `.pdf`: the path comes from the thread and must not launch anything else.
#[tauri::command]
fn open_pdf(app: tauri::AppHandle, path: String) -> Result<(), IpcError> {
    let file = std::path::Path::new(&path);
    let pdf = file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
    if !pdf || !file.is_absolute() || !file.is_file() {
        return Err(IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Invalid,
            message: format!("only a PDF file opens here: {path}"),
        });
    }
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(|err| IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Internal,
            message: format!("could not open it: {err}"),
        })
}

/// Shows a file (a file link in an answer) selected in the system file manager.
#[tauri::command]
fn reveal_path(app: tauri::AppHandle, path: String) -> Result<(), IpcError> {
    app.opener()
        .reveal_item_in_dir(path)
        .map_err(|err| IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Internal,
            message: format!("could not show it: {err}"),
        })
}

/// The app became usable at `ready_ms` (ms since the Unix epoch): its loaded catalog painted
/// and the startup screen gone. Returns cold start: process start to then.
#[tauri::command]
fn app_ready(state: State<'_, AppState>, ready_ms: f64) -> f64 {
    let mut cold_start = state.cold_start_ms.lock().expect("cold start lock");
    *cold_start.get_or_insert(ready_ms - state.info.process_start_ms)
}

/// Desktop notifications from the app itself, so they show as Brigadier. Best effort.
fn notifier(app: tauri::AppHandle) -> Notify {
    Box::new(move |title, body| {
        let shown = app.notification().builder().title(title).body(body).show();
        if let Err(err) = shown {
            tracing::debug!(error = %err, "could not show a desktop notification");
        }
    })
}

/// Opens computer use's setup window for an agent.
fn open_setup(app: tauri::AppHandle) -> bridge::OpenSetup {
    Box::new(move || computer_setup::open(&app, false))
}

/// Whether a Brigadier window is shown and focused, for the notifications that show only
/// while it is in the background.
fn in_front(app: tauri::AppHandle) -> bridge::InFront {
    Box::new(move || {
        app.webview_windows().values().any(|window| {
            window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
        })
    })
}

/// The window's backdrop behind the startup screen (the blur on macOS), from the config.
fn startup_backdrop(app: &tauri::AppHandle) -> Option<tauri::utils::config::WindowEffectsConfig> {
    app.config()
        .app
        .windows
        .iter()
        .find(|window| window.label == shell::MAIN_WINDOW)
        .and_then(|window| window.window_effects.clone())
}

/// The window's colour once the app covers it, as before the startup screen had a backdrop
/// (`backgroundColor` in tauri.conf.json): a resize never shows through to the desktop.
pub(crate) const WINDOW_BACKGROUND: tauri::window::Color =
    tauri::window::Color(0x0f, 0x0f, 0x0e, 0xff);

/// Takes the startup backdrop off `window`. Tauri's `set_effects(None)` clears it only on
/// Windows, so on macOS the blur views it added are removed here: all of them, so none is left
/// behind the app.
fn clear_backdrop(window: &tauri::Window) {
    #[cfg(target_os = "macos")]
    let result = {
        let handle = window.clone();
        window.run_on_main_thread(move || {
            loop {
                match window_vibrancy::clear_vibrancy(&handle) {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(err) => {
                        tracing::warn!(error = %err, "could not clear the startup backdrop");
                        break;
                    }
                }
            }
        })
    };
    #[cfg(not(target_os = "macos"))]
    let result = window.set_effects(None);
    if let Err(err) = result {
        tracing::warn!(error = %err, "could not clear the startup backdrop");
    }
}

/// The startup screen has gone and the app covers the window: its backdrop goes too.
#[tauri::command]
fn startup_finished(app: tauri::AppHandle, window: tauri::Window) {
    // A staged intro keeps the window clear until its burst has opened it (intro.rs).
    if startup_backdrop(&app).is_some() && !intro::staged() {
        clear_backdrop(&window);
        if let Err(err) = window.set_background_color(Some(WINDOW_BACKGROUND)) {
            tracing::warn!(error = %err, "could not set the window's colour");
        }
    }
}

/// Completes the smoke check with the webview's measurements.
#[tauri::command]
async fn smoke_finish(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    measurements: UiMeasurements,
) -> Result<SmokeReport, IpcError> {
    if !state.info.smoke {
        return Err(IpcError {
            code: brigadier_ipc::protocol::ErrorCode::Invalid,
            message: "not running the smoke check".into(),
        });
    }
    let diagnostics = match state.bridge.request(Request::GetDiagnostics).await? {
        Response::GetDiagnostics { diagnostics } => diagnostics,
        _ => {
            return Err(IpcError {
                code: brigadier_ipc::protocol::ErrorCode::Internal,
                message: "unexpected response".into(),
            });
        }
    };
    let cold_start_ms = state
        .cold_start_ms
        .lock()
        .expect("cold start lock")
        .unwrap_or(f64::INFINITY);
    let report = smoke::evaluate(
        &state.info.platform,
        state.info.budget_tolerance,
        cold_start_ms,
        state.info.first_launch,
        &measurements,
        &diagnostics,
    );
    smoke::finish(&app, &report);
    Ok(report)
}

/// Where a debug build keeps its data when it is given no data directory: never the installed
/// app's.
fn debug_data_dir() -> std::path::PathBuf {
    if cfg!(unix) {
        std::path::PathBuf::from("/tmp/brigadier-dev")
    } else {
        std::env::temp_dir().join("brigadier-dev")
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BRIGADIER_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    #[cfg(target_os = "macos")]
    if !shell::window_server_reachable() {
        eprintln!(
            "brigadier: no window server here (a sandbox or an SSH session), so the app and \
             --smoke can't start; run it from the logged-in desktop session"
        );
        std::process::exit(NO_WINDOW_SERVER);
    }

    let args: Vec<_> = std::env::args().collect();
    let smoke = args.iter().any(|arg| arg == "--smoke");
    let mut context = tauri::generate_context!();
    let data_dir = args
        .windows(2)
        .find(|pair| pair[0] == "--brigadier-data-dir")
        .map(|pair| std::path::PathBuf::from(&pair[1]));
    // Nor does a debug build use the installed app's data: without a data directory of its
    // own it gets a throwaway one.
    let data_dir = data_dir.or_else(|| {
        (cfg!(debug_assertions) && std::env::var_os(brigadier_sandbox::DATA_DIR_ENV).is_none())
            .then(debug_data_dir)
    });
    let platform = match brigadier_sandbox::native_for_identifier(
        &context.config().identifier,
        PlatformOptions { data_dir },
    ) {
        Ok(platform) => platform,
        Err(err) => {
            eprintln!("brigadier: {err}");
            std::process::exit(1);
        }
    };
    let process_start_ms = platform
        .processes()
        .start_time_ms(std::process::id())
        .unwrap_or_else(|_| brigadier_core::now_ms() as f64);
    let budget_tolerance = std::env::var(TOLERANCE_ENV)
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 1.0)
        .unwrap_or(1.0);
    // Before the daemon is launched, which creates the database.
    let first_launch = !platform.paths().db_path.exists();
    let info = AppInfo {
        version: updates::version(context.config()),
        platform: platform.name().into(),
        process_start_ms,
        build: updates::build(context.config()),
        update_channel: updates::channel(context.config()).map(str::to_owned),
        smoke,
        first_launch,
        budget_tolerance,
    };
    // The daemon applies the same tolerance to its stall threshold.
    let daemon_env = std::env::var(TOLERANCE_ENV)
        .map(|value| vec![(TOLERANCE_ENV.to_owned(), value)])
        .unwrap_or_default();

    // A debug build never takes the installed app's identity, even when built without
    // `pnpm tauri:dev` / `tauri:debug-app` (which give the bundle that identity too): a launch
    // would otherwise be handed to the installed app by the single-instance lock.
    if cfg!(debug_assertions) && context.config().identifier == RELEASE_IDENTIFIER {
        context.config_mut().identifier = DEBUG_IDENTIFIER.to_owned();
    }
    // A session's app preview (#142): the user's work keeps the focus, at this launch and
    // whenever it shows its window later, until the user comes to it.
    let background = std::env::var_os(brigadier_sandbox::BACKGROUND_LAUNCH_ENV)
        .is_some_and(|value| value == "1");
    if background {
        shell::launch_in_background();
        for window in &mut context.config_mut().app.windows {
            window.focus = false;
        }
    }
    // The first launch's intro shows the window itself, once the page is ready (intro.rs).
    let hold_for_intro = intro::should_hold(first_launch, smoke);
    if hold_for_intro {
        for window in &mut context.config_mut().app.windows {
            window.visible = false;
        }
    }
    // Only the signed nightly carries the updater's configuration (updates.rs).
    let check_updates = updates::enabled(
        cfg!(debug_assertions),
        updates::configured(context.config()),
        smoke,
    );
    tracing::info!(on = check_updates, "nightly updates");
    let builder = tauri::Builder::default().register_uri_scheme_protocol("brigadier-ui", |_context, request| {
        let found = request.uri().path() == "/frame";
        tauri::http::Response::builder()
            .status(if found { 200 } else { 404 })
            .header("Content-Type", "text/html; charset=utf-8")
            .header("Content-Security-Policy", "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-src 'none'; object-src 'none'")
            .body(if found { include_bytes!("visuals_frame.html").to_vec() } else { Vec::new() })
            .expect("fixed visual frame response")
    });
    let builder = if check_updates {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    } else {
        builder
    };
    let app = builder
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            shell::show_main(app);
            open_folders(
                app,
                folder_args(args.into_iter().skip(1), std::path::Path::new(&cwd)),
            );
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            browser::set_data_dir(&platform.paths().data_dir);
            let launcher = Launcher::new(platform.clone(), daemon_env);
            let bridge = Bridge::start(
                platform.clone() as Arc<dyn Platform>,
                launcher,
                notifier(app.handle().clone()),
                in_front(app.handle().clone()),
                open_setup(app.handle().clone()),
            );
            computer_menu::install(app.handle());
            app.manage(documents::Documents::default());
            app.manage(AppState {
                bridge,
                info,
                cold_start_ms: Mutex::new(None),
            });
            if hold_for_intro {
                intro::hold(app.handle());
            }
            if let Ok(cwd) = std::env::current_dir() {
                open_folders(app.handle(), folder_args(std::env::args().skip(1), &cwd));
            }
            #[cfg(target_os = "macos")]
            if let Err(err) = shell::install_app_menu(app.handle()) {
                tracing::warn!(error = %err, "could not add File › Open Folder…");
            }
            #[cfg(windows)]
            if let Some(window) = app.get_webview_window(shell::MAIN_WINDOW) {
                let window = window.as_ref().window();
                shell::set_window_icon(&window, window.scale_factor().unwrap_or(1.0));
            }
            // No menu-bar host (e.g. a bare Linux session) must not stop the app.
            if let Err(err) = shell::install_tray(app.handle()) {
                tracing::warn!(error = %err, "menu-bar item unavailable");
            }
            if smoke {
                smoke::start_watchdog(app.handle());
            }
            if check_updates {
                updates::start(app.handle().clone());
            }
            Ok(())
        })
        .on_page_load(|webview, payload| {
            if webview.label() == shell::MAIN_WINDOW
                && payload.event() == tauri::webview::PageLoadEvent::Started
            {
                // A new page has no intro playing: one staged for the last ends here.
                intro::interrupt(webview.app_handle());
                // A new page in the app's webview knows none of the Browser tabs' pages: drop them.
                let _ = webview.app_handle().run_on_main_thread(browser::close_all);
                // It starts on the startup screen again, which needs the backdrop behind it
                // (once: applying it again adds another blur view on macOS).
                if let Some(backdrop) = startup_backdrop(webview.app_handle()) {
                    let window = webview.window();
                    // Clear again (a transparent window's default), so the backdrop shows.
                    if let Err(err) = window.set_background_color(None) {
                        tracing::warn!(error = %err, "could not clear the window's colour");
                    }
                    clear_backdrop(&window);
                    if let Err(err) = window.set_effects(backdrop) {
                        tracing::warn!(error = %err, "could not restore the startup backdrop");
                    }
                }
            }
        })
        .on_window_event(|window, event| {
            // Moved to a screen of another scale, the window's icon is drawn for that one.
            #[cfg(windows)]
            if let WindowEvent::ScaleFactorChanged { scale_factor, .. } = event
                && window.label() == shell::MAIN_WINDOW
            {
                shell::set_window_icon(window, *scale_factor);
            }
            if let WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
                drops::record(paths);
            } else if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == shell::MAIN_WINDOW
                && !shell::is_quitting()
            {
                // Closing the window keeps Brigadier running in the menu bar.
                api.prevent_close();
                shell::close_main(window.app_handle());
            } else if let WindowEvent::CloseRequested { api, .. } = event
                && window.label() == computer_setup::SETUP_WINDOW
                && !shell::is_quitting()
            {
                // Closed, the setup window is only hidden, and opens again from there: a closed
                // one would stay behind as a hidden window each time.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            reports::get_continuity_report,
            documents::save_document,
            downloads::save_copy,
            drops::drag_shift,
            drops::last_drop,
            drops::read_dropped,
            documents::forget_document,
            app_info,
            uninstall_app,
            quit_app,
            pages_flushed,
            ipc_request,
            ipc_subscribe,
            notification_permission,
            ask_notification_permission,
            open_notification_settings,
            login_item::login_item,
            login_item::set_login_item,
            login_item::open_login_items,
            app_ready,
            startup_finished,
            intro::intro_begin,
            intro::intro_shaped,
            intro::intro_finish,
            intro::intro_release,
            smoke_finish,
            pick_folder,
            warm_folder_picker,
            pick_folders,
            pick_file,
            take_opened_folders,
            save_artifact,
            export_conventions,
            open_artifact,
            open_folder,
            open_url,
            open_pdf,
            simulator::open_simulator,
            system_name,
            reveal_path,
            set_running_chats,
            set_key_capture,
            computer_setup::open_privacy_settings,
            computer_menu::set_computer_menu,
            computer_setup::open_computer_setup,
            computer_setup::computer_setup_subscribe,
            computer_setup::computer_setup_drag,
            computer_setup::reveal_computer_helper,
            browser::browser_configure,
            browser::browser_open,
            browser::browser_navigate,
            browser::browser_place,
            browser::browser_layer,
            browser::browser_permission,
            browser::browser_go,
            browser::browser_zoom,
            browser::browser_find,
            browser::browser_read,
            browser_agent::browser_agent,
            browser_agent::browser_agent_load,
            browser_agent::browser_agent_capture,
            browser::browser_print,
            browser::browser_snapshot,
            browser::browser_focus,
            browser::browser_close,
            browser::browser_history,
            browser::browser_history_remove,
            browser::browser_clear,
            browser::browser_site_count,
            browser::browser_downloads,
            browser::browser_download_open,
            browser::browser_downloads_remove,
            browser::browser_sites,
            browser::browser_site_set,
            browser_import::browser_import_sources,
            browser_import::browser_import,
            browser_import::browser_import_open_access,
            updates::app_update,
            updates::check_for_updates,
            updates::take_app_update,
            updates::restart_to_update
        ])
        .build(context)
        .unwrap_or_else(|err| {
            eprintln!("brigadier: failed to start: {err}");
            std::process::exit(1);
        });

    let env = app.env();
    app.run_return(|app, event| match event {
        // Cmd+Q and other user-initiated exits go through the orderly quit.
        RunEvent::ExitRequested { api, code, .. } if code.is_none() && !shell::is_quitting() => {
            api.prevent_exit();
            shell::quit(app, 0);
        }
        RunEvent::Exit => shell::quit_on_exit(app),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => {
            shell::user_came_to_it();
            shell::show_main(app);
        }
        #[cfg(target_os = "macos")]
        RunEvent::Opened { urls } => open_folders(
            app,
            urls.into_iter().filter_map(|url| url.to_file_path().ok()),
        ),
        _ => {}
    });
    // A waiting nightly update went in at the last event, after the daemon stopped; "Restart
    // to update" opens the app again.
    if updates::finish(shell::daemon_stopped()) {
        tauri::process::restart(&env);
    }
    // The code the quit asked for, so smoke failures reach CI.
    std::process::exit(shell::exit_code());
}

#[cfg(test)]
mod tests {
    use super::folder_args;

    /// A folder named on the command line is opened; the data folder given with
    /// `--brigadier-data-dir` is not (issues #67, #97).
    #[test]
    fn the_data_dir_option_s_value_is_no_folder_to_open() {
        let root =
            std::env::temp_dir().join(format!("brigadier-folder-args-{}", std::process::id()));
        let (data, repo) = (root.join("data"), root.join("repo"));
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&repo).unwrap();
        let args = [
            "--brigadier-data-dir",
            data.to_str().unwrap(),
            "repo",
            "--smoke",
        ]
        .map(String::from);
        let folders = folder_args(args, &root);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(folders, vec![repo]);
    }
}
