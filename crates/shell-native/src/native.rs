//! The Node addon the Electron main process loads. JSON crosses as strings: the bridge's events and
//! responses are the app's hottest path, and main only passes them on to the page unread.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_ipc::app::{BridgeEvent, UiMeasurements};
use brigadier_ipc::metrics::Diagnostics;
use brigadier_ipc::protocol::Request;
use brigadier_sandbox::{Platform, PlatformOptions};
use napi::bindgen_prelude::{Result, Status};
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

use crate::bridge::{Bridge, Hooks, Sink};
use crate::launcher::Launcher;
use crate::opening::{self, Opening};

/// A JavaScript callback taking one JSON string, callable from any thread, in order.
type JsonCallback = ThreadsafeFunction<String, (), String, Status, false>;

fn send(callback: &JsonCallback, json: String) {
    let status = callback.call(json, ThreadsafeFunctionCallMode::NonBlocking);
    if status != Status::Ok {
        tracing::warn!(?status, "could not reach the Electron main process");
    }
}

fn sink(callback: JsonCallback) -> Sink {
    Box::new(
        move |event: BridgeEvent| match serde_json::to_string(&event) {
            Ok(json) => send(&callback, json),
            Err(err) => tracing::error!(error = %err, "could not serialize a bridge event"),
        },
    )
}

fn failed(message: impl std::fmt::Display) -> napi::Error {
    napi::Error::from_reason(message.to_string())
}

struct Running {
    bridge: Bridge,
    platform: Arc<dyn Platform>,
}

static RUNNING: Mutex<Option<Running>> = Mutex::new(None);
/// Read on the bridge's threads, where asking JavaScript would wait on a busy main process.
static IN_FRONT: AtomicBool = AtomicBool::new(false);
static QUITTING: AtomicBool = AtomicBool::new(false);

fn bridge() -> Result<Bridge> {
    RUNNING
        .lock()
        .expect("running lock")
        .as_ref()
        .map(|running| running.bridge.clone())
        .ok_or_else(|| failed("the shell has not started"))
}

#[napi(object)]
pub struct StartOptions {
    pub identifier: String,
    pub data_dir: Option<String>,
    /// Where `brigadierd` is, when not beside this executable.
    pub sidecar_dir: Option<String>,
    /// Passed on to the daemon.
    pub daemon_env: Vec<Vec<String>>,
}

#[napi(object)]
pub struct Started {
    pub platform: String,
    /// When the OS started this process, in ms since the Unix epoch.
    pub process_start_ms: f64,
    /// No database yet: judged against the first-launch budget.
    pub first_launch: bool,
    pub data_dir: String,
}

/// Starts the bridge to the daemon. `on_shell` hears what the bridge asks of the shell, as JSON:
/// `{"kind":"notify","title","body"}`, `{"kind":"openSetup"}`, `{"kind":"computerUse","status"}`.
#[napi]
pub async fn start(options: StartOptions, on_shell: JsonCallback) -> Result<Started> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("BRIGADIER_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();
    let platform = brigadier_sandbox::native_for_identifier(
        &options.identifier,
        PlatformOptions {
            data_dir: options.data_dir.map(PathBuf::from),
        },
    )
    .map_err(failed)?;
    let process_start_ms = platform
        .processes()
        .start_time_ms(std::process::id())
        .unwrap_or_else(|_| brigadier_core::now_ms() as f64);
    // Before the daemon is launched, which creates the database.
    let first_launch = !platform.paths().db_path.exists();
    let data_dir = platform.paths().data_dir.clone();
    crate::browser_store::set_data_dir(&data_dir);
    let daemon_env = options
        .daemon_env
        .into_iter()
        .filter_map(|pair| {
            <[String; 2]>::try_from(pair)
                .ok()
                .map(|[key, value]| (key, value))
        })
        .collect();
    let mut launcher = Launcher::new(platform.clone(), daemon_env);
    if let Some(dir) = options.sidecar_dir {
        launcher = launcher.with_sidecar_dir(PathBuf::from(dir));
    }
    let on_shell = Arc::new(on_shell);
    let notify = {
        let on_shell = on_shell.clone();
        Box::new(move |title: &str, body: &str| {
            let json = serde_json::json!({ "kind": "notify", "title": title, "body": body });
            send(&on_shell, json.to_string());
        })
    };
    let open_setup = {
        let on_shell = on_shell.clone();
        Box::new(move || send(&on_shell, r#"{"kind":"openSetup"}"#.to_owned()))
    };
    let computer_use_changed = Box::new(move |status: &brigadier_core::ComputerUseStatus| {
        let json = serde_json::json!({ "kind": "computerUse", "status": status });
        send(&on_shell, json.to_string());
    });
    let bridge = Bridge::start(
        platform.clone() as Arc<dyn Platform>,
        launcher,
        Hooks {
            notify,
            in_front: Box::new(|| IN_FRONT.load(Ordering::Acquire)),
            open_setup,
            computer_use_changed,
            is_quitting: Box::new(|| QUITTING.load(Ordering::Acquire)),
        },
    );
    let started = Started {
        platform: platform.name().to_owned(),
        process_start_ms,
        first_launch,
        data_dir: data_dir.display().to_string(),
    };
    *RUNNING.lock().expect("running lock") = Some(Running {
        bridge,
        platform: platform as Arc<dyn Platform>,
    });
    Ok(started)
}

/// A request's outcome: exactly one of the response and the error, each JSON.
#[napi(object)]
pub struct RequestOutcome {
    pub response: Option<String>,
    pub error: Option<String>,
}

/// Sends a request to the daemon. A daemon's error comes back as data, so its code reaches the page.
#[napi]
pub async fn request(json: String) -> Result<RequestOutcome> {
    let request: Request = serde_json::from_str(&json).map_err(failed)?;
    let outcome = bridge()?.request(request).await;
    Ok(match outcome {
        Ok(response) => RequestOutcome {
            response: Some(serde_json::to_string(&response).map_err(failed)?),
            error: None,
        },
        Err(error) => RequestOutcome {
            response: None,
            error: Some(serde_json::to_string(&error).map_err(failed)?),
        },
    })
}

/// Routes bridge events to the main window's page, replacing the feed of a page that went.
#[napi]
pub fn attach_ui(on_event: JsonCallback) -> Result<()> {
    bridge()?.attach_ui(sink(on_event));
    Ok(())
}

/// Routes computer use's status to the setup window's page.
#[napi]
pub fn attach_setup_ui(on_event: JsonCallback) -> Result<()> {
    bridge()?.attach_setup_ui(sink(on_event));
    Ok(())
}

/// Sends the page a bridge event of the shell's own: a menu item, the window's visibility.
#[napi]
pub fn emit(json: String) -> Result<()> {
    let event: BridgeEvent = serde_json::from_str(&json).map_err(failed)?;
    bridge()?.emit(event);
    Ok(())
}

/// A native activation for a conversation, kept until the page is ready for it.
#[napi]
pub fn open_conversation(conversation_id: String) -> Result<()> {
    bridge()?.open_conversation(conversation_id);
    Ok(())
}

/// Asks the daemon to drain and exit; false if it couldn't be reached in time.
#[napi]
pub async fn shutdown_daemon(timeout_ms: u32) -> Result<bool> {
    Ok(bridge()?
        .shutdown_daemon(Duration::from_millis(u64::from(timeout_ms)))
        .await)
}

/// Whether a Brigadier window is shown and focused, for notifications only shown in the background.
#[napi]
pub fn set_in_front(in_front: bool) {
    IN_FRONT.store(in_front, Ordering::Release);
}

/// The app is quitting: the bridge raises no more notices.
#[napi]
pub fn set_quitting(quitting: bool) {
    QUITTING.store(quitting, Ordering::Release);
}

/// What may happen to a path the thread or a worker named (#770).
#[napi(object)]
pub struct OpeningDecision {
    /// `open`, `reveal` or `refuse`.
    pub kind: String,
    /// What was refused or revealed: a reason, or a file's name.
    pub detail: Option<String>,
}

#[napi]
pub fn decide_opening(target: String, with_app: bool) -> OpeningDecision {
    match opening::decide(&target, with_app) {
        Opening::Open => OpeningDecision {
            kind: "open".to_owned(),
            detail: None,
        },
        Opening::Reveal(name) => OpeningDecision {
            kind: "reveal".to_owned(),
            detail: Some(name),
        },
        Opening::Refuse(why) => OpeningDecision {
            kind: "refuse".to_owned(),
            detail: Some(why),
        },
    }
}

/// Judges a smoke run against the budgets; the report as JSON.
#[napi]
pub fn evaluate_smoke(
    tolerance: f64,
    cold_start_ms: f64,
    first_launch: bool,
    measurements: String,
    diagnostics: String,
) -> Result<String> {
    let platform = RUNNING
        .lock()
        .expect("running lock")
        .as_ref()
        .map(|running| running.platform.name().to_owned())
        .ok_or_else(|| failed("the shell has not started"))?;
    let measurements: UiMeasurements = serde_json::from_str(&measurements).map_err(failed)?;
    let diagnostics: Diagnostics = serde_json::from_str(&diagnostics).map_err(failed)?;
    let report = crate::smoke::evaluate(
        &platform,
        tolerance,
        cold_start_ms,
        first_launch,
        &measurements,
        &diagnostics,
    );
    serde_json::to_string(&report).map_err(failed)
}

/// This computer's system and processor, for feedback ("macOS 27.0 Tahoe (aarch64)").
#[napi]
pub fn system_name() -> String {
    let system =
        sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.to_owned());
    format!("{system} ({})", std::env::consts::ARCH)
}

/// The latest continuity report (Settings › Reports), as JSON.
#[napi]
pub async fn continuity_report() -> Result<String> {
    let report = crate::reports::get_continuity_report()
        .await
        .map_err(failed)?;
    serde_json::to_string(&report).map_err(failed)
}

/// Opens the Simulator app on a preview's device; rejects with an IpcError as JSON.
#[napi]
pub async fn open_simulator(device: Option<String>) -> Result<()> {
    crate::simulator::open_simulator(device)
        .await
        .map_err(|error| {
            failed(serde_json::to_string(&error).unwrap_or_else(|_| error.message.clone()))
        })
}
