//! Brigadier's `launch_app`, in place of the driver's.
//!
//! An app opens behind the user's work and never takes the front:
//!
//! - an app not running yet starts hidden, then shows without activating;
//!   if it comes forward anyway it is sent back at once through the
//!   WindowServer, also while the user is typing, and kept back for about
//!   ten seconds (an Electron app activates itself once its first window is
//!   ready);
//! - a web link or local page for a browser that is already running opens
//!   in a new background window of its own ([`crate::browser::open_link`]),
//!   listed first;
//! - a file or link for an app that is already running opens in it, also
//!   when given among `additional_arguments`, and its new window is listed
//!   first;
//! - an app that is still quitting is opened once it is gone.
//!
//! Every suppression goes through the driver's focus-steal preventer, so
//! Brigadier's restore policy ([`crate::focus_policy`]) decides each restore,
//! and every send-back this tool makes itself asks that policy first.

pub mod apps;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use platform_macos::focus_steal::{
    begin_suppression, restore_allowed, RestoreKind, RestoreRequest, SuppressionLease,
};
use platform_macos::windows::WindowInfo;
use serde_json::{json, Value};

use crate::browser::open_link::{self, OpenedBy};

/// Replace the driver's `launch_app` with Brigadier's. Call it before
/// [`crate::guard::install`], so the guard wraps this one.
pub fn install(registry: &mut ToolRegistry) {
    registry.wrap("launch_app", |_driver_launch_app| {
        Box::new(LaunchAppTool) as Box<dyn Tool>
    });
}

/// Bundle id of System Settings.
pub const SETTINGS_BUNDLE_ID: &str = "com.apple.systempreferences";

/// The line a launch of System Settings without urls adds to its answer.
pub const SETTINGS_HINT: &str = "System Settings can be opened and read, never clicked or typed \
     into. To read a pane, open it directly: launch_app with bundle_id \
     com.apple.systempreferences and urls [\"x-apple.systempreferences:<pane id>\"], for \
     example com.apple.SystemProfiler.AboutExtension (General › About).";

/// How long a launch waits for an earlier instance of the app to finish
/// quitting.
const QUITTING_WAIT: Duration = Duration::from_secs(3);

/// One watchdog tick.
const TICK: Duration = Duration::from_millis(250);

/// The driver ends a suppression after 5 s; a longer watch takes a fresh one
/// this often, before the last one ends.
const LEASE_RENEWAL: Duration = Duration::from_secs(4);

/// How long the call itself waits for an activation before answering; the
/// watchdog holds on after it.
const IN_CALL_WAIT: Duration = Duration::from_millis(60);

pub struct LaunchAppTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "launch_app".into(),
        description:
            "Launch a macOS app in the background — the target does NOT come to the foreground.\n\n\
             Provide either `bundle_id` (preferred — unambiguous, e.g. `com.apple.calculator`) \
             or `name` (e.g. \"Calculator\"). If both are given, bundle_id wins.\n\n\
             Optional `urls` are handed to the app as open targets — for Finder, pass a folder \
             path to open a backgrounded Finder window there. A web browser that is already \
             running opens web links and local pages in a new background window of its own, listed first in \
             `windows`.\n\n\
             Optional `cdp_debugging_port`: opens a Chrome DevTools Protocol (CDP) server \
             on the specified port (appends --remote-debugging-port=N to the app's argv). \
             Use this to automate Electron/VS Code/Cursor, or Chrome/Brave/Edge, via CDP.\n\n\
             Optional `webkit_inspector_port`: opens a WebKit inspector server on the specified \
             port (sets WEBKIT_INSPECTOR_SERVER=127.0.0.1:N + TAURI_WEBVIEW_AUTOMATION=1). \
             Use this for Tauri/WebKit-based apps.\n\n\
             Optional `creates_new_application_instance`: when true, forces a new app instance \
             even if one is already running (passes -n to open). Reach for this when another \
             agent or session may drive the SAME app concurrently — it returns a fresh pid + \
             window so each session acts on its own isolated window instead of clobbering one \
             shared instance. Without it, single-instance apps (Calculator, many utilities) hand \
             every caller the same window, so two sessions fight over it.\n\n\
             Optional `additional_arguments`: extra argv strings appended after --args.\n\n\
             Returns the launched app's pid, bundle_id, name, and a `windows` array \
             (same shape as `list_windows`) so callers can skip an extra round-trip before \
             `get_window_state(pid, window_id)`. When the focus-steal belt-and-braces \
             demotion check ran (target pid ≠ prior frontmost), the response also includes \
             `self_activation_suppressed: bool` — true if focus stayed with the prior \
             frontmost, false if the launched app held focus despite the re-demote attempt."
            .into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "bundle_id": {
                    "type": "string",
                    "description": "App bundle identifier, e.g. com.apple.calculator. Preferred over name."
                },
                "name": {
                    "type": "string",
                    "description": "App display name, or the full path to its .app bundle. Used only when bundle_id is absent."
                },
                "urls": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional file paths or URLs to open with the app (e.g. a folder path for Finder)."
                },
                "cdp_debugging_port": {
                    "type": "integer",
                    "description": "Open a Chrome DevTools Protocol server on this port (appends --remote-debugging-port=N)."
                },
                "webkit_inspector_port": {
                    "type": "integer",
                    "description": "Open a WebKit inspector server on this port (sets WEBKIT_INSPECTOR_SERVER env var)."
                },
                "creates_new_application_instance": {
                    "type": "boolean",
                    "description": "When true, force a new app instance even if already running (open -n). Use for concurrent multi-agent/multi-session work so each session gets an isolated instance + window instead of sharing one — on single-instance apps (e.g. Calculator) every caller otherwise gets the same window and the sessions clobber each other."
                },
                "additional_arguments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra arguments appended after --args when launching."
                }
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: true,
        open_world: true,
    })
}

/// What the blocking launch hands back.
struct Launched {
    pid: i32,
    app: Option<apps::RunningApp>,
    windows: Vec<WindowInfo>,
}

#[async_trait]
impl Tool for LaunchAppTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let bundle_id = args.opt_str("bundle_id");
        let name = args.opt_str("name");
        let mut urls: Vec<String> = args.str_array("urls");
        let cdp_debugging_port = args.opt_u64("cdp_debugging_port").map(|v| v as u16);
        let webkit_inspector_port = args.opt_u64("webkit_inspector_port").map(|v| v as u16);
        let creates_new_instance = args.bool_or("creates_new_application_instance", false);
        let mut additional_arguments: Vec<String> = args.str_array("additional_arguments");

        if bundle_id.is_none() && name.is_none() {
            return ToolResult::error(
                "Provide either bundle_id or name to identify the app to launch.",
            );
        }
        if let Some(ref bid) = bundle_id {
            if !apps::bundle_installed(bid) {
                return structured_launch_error(
                    "APP_NOT_INSTALLED",
                    format!("No installed macOS app found for bundle_id '{bid}'."),
                    json!({ "bundle_id": bid }),
                );
            }
        } else if let Some(ref n) = name {
            if apps::locate_by_name(n).is_none() {
                return structured_launch_error(
                    "APP_NOT_INSTALLED",
                    format!("No installed macOS app found for name '{n}'."),
                    json!({ "name": n }),
                );
            }
        }
        let windows_before: HashSet<u32> = platform_macos::windows::all_windows()
            .into_iter()
            .map(|w| w.window_id)
            .collect();
        let running_before = apps::running_apps();
        let pids_before: HashSet<i32> = running_before.iter().map(|a| a.pid).collect();
        let is_named = |app: &apps::RunningApp| {
            names_app(
                app.bundle_id.as_deref(),
                &app.name,
                bundle_id.as_deref(),
                name.as_deref(),
            )
        };

        // A running app keeps the arguments it started with, so a file or
        // link given among them would be dropped without a word: open it as
        // a url.
        let mut moved_targets = Vec::new();
        if !creates_new_instance
            && !additional_arguments.is_empty()
            && running_before.iter().any(is_named)
        {
            let (targets, rest) = split_open_targets(additional_arguments);
            additional_arguments = rest;
            urls.extend(targets.iter().cloned());
            moved_targets = targets;
        }
        if let Some(err) = preflight_file_urls(&urls) {
            return err;
        }

        if let Some(port) = cdp_debugging_port {
            additional_arguments.push(format!("--remote-debugging-port={port}"));
        }
        let mut env: HashMap<String, String> = HashMap::new();
        if let Some(port) = webkit_inspector_port {
            env.insert(
                "WEBKIT_INSPECTOR_SERVER".to_string(),
                format!("127.0.0.1:{port}"),
            );
            env.insert("TAURI_WEBVIEW_AUTOMATION".to_string(), "1".to_string());
        }
        let port_summary = {
            let mut s = String::new();
            if let Some(port) = cdp_debugging_port {
                s.push_str(&format!("\nCDP renderer available on port {port}."));
            }
            if let Some(port) = webkit_inspector_port {
                s.push_str(&format!("\nWebKit inspector available on port {port}."));
            }
            s
        };

        // Suppression in three steps: a wildcard lease before the launch
        // (an app may activate itself while LaunchServices is still
        // opening it), a lease for the launched pid once it is known, armed
        // before the wildcard one ends, and a watchdog that holds on after
        // the call has answered.
        let prior_frontmost = apps::front_pid();
        let action_started = Instant::now();
        let wildcard_lease =
            prior_frontmost.map(|prior| begin_suppression(None, prior, "LaunchAppTool.pre"));

        // A web browser that is already running is asked to open the links
        // itself, in a new window it keeps in the background, instead of
        // through LaunchServices, which brings it to the front.
        let opened_in_background = if creates_new_instance {
            None
        } else {
            open_link::open_in_running_browser(
                bundle_id.as_deref(),
                name.as_deref(),
                &urls,
                cdp_debugging_port,
            )
            .await
        };
        let link_count = urls.len();

        // A launch that hands the app documents, arguments or a new instance
        // gets a second activation when the documents arrive, later than a
        // plain launch's. Links the browser opened itself bring none.
        let slow_launch_path = opened_in_background.is_none()
            && (!urls.is_empty()
                || !additional_arguments.is_empty()
                || !env.is_empty()
                || creates_new_instance);

        let windows_seen = windows_before.clone();
        let launch_result = tokio::task::spawn_blocking(move || {
            if let Some(opened) = opened_in_background {
                let pid = opened.pid;
                let (mut windows, new) =
                    open_link::wait_for_new_windows(pid, &windows_seen, urls.len());
                if new == 0 {
                    put_window_first(&mut windows, apps::main_window_id(pid));
                }
                crate::offspace::remember_front_windows(pid);
                let app = apps::running_apps().into_iter().find(|a| a.pid == pid);
                return Ok(Launched { pid, app, windows });
            }
            let named = |app: &apps::RunningApp| {
                names_app(
                    app.bundle_id.as_deref(),
                    &app.name,
                    bundle_id.as_deref(),
                    name.as_deref(),
                )
            };
            // An app already running opens the documents in windows of its
            // own, often only after LaunchServices returns; it is watched
            // first so each one is caught as it opens, also on another Space.
            let running = (!urls.is_empty())
                .then(|| apps::running_apps().into_iter().find(|app| named(app)))
                .flatten();
            if let Some(app) = &running {
                crate::offspace::watch(app.pid);
            }
            let opens_in_running_app = running.is_some() && !creates_new_instance;
            let request = apps::Launch {
                bundle_id: bundle_id.as_deref(),
                name: name.as_deref(),
                urls: &urls,
                arguments: &additional_arguments,
                environment: &env,
                creates_new_instance,
            };
            // An app that is still quitting (asked to quit just before) can't
            // be launched until it is gone: try again while it goes.
            let deadline = Instant::now() + QUITTING_WAIT;
            let mut retried = false;
            let pid = loop {
                match apps::launch(&request) {
                    Ok(pid) => break pid,
                    Err(error) => {
                        let still_running = apps::running_apps().iter().any(named);
                        if !retry_launch(Instant::now() >= deadline, retried, still_running) {
                            return Err(error);
                        }
                        retried = true;
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            };
            let windows = if opens_in_running_app {
                // Wait a little for the documents' windows and list them
                // first, so the caller needn't list the windows again.
                let (mut windows, new) =
                    open_link::wait_for_new_windows(pid, &windows_seen, urls.len());
                if new == 0 {
                    put_window_first(&mut windows, apps::main_window_id(pid));
                }
                windows
            } else {
                let mut windows = resolve_windows_for_pid(pid);
                put_window_first(&mut windows, apps::main_window_id(pid));
                windows
            };
            crate::offspace::remember_front_windows(pid);
            let app = apps::running_apps().into_iter().find(|a| a.pid == pid);
            Ok::<_, anyhow::Error>(Launched { pid, app, windows })
        })
        .await;

        // `self_activation_suppressed`: `None` when the check didn't run (no
        // prior frontmost, the launch failed, or the app was already in
        // front), else whether the launched app stayed behind.
        let mut self_activation_suppressed: Option<bool> = None;
        match (&launch_result, prior_frontmost) {
            (Ok(Ok(launched)), Some(prior)) if launched.pid != prior => {
                let pid = launched.pid;
                let cold_start = !pids_before.contains(&pid);
                let lasting = TICK * watchdog_ticks(slow_launch_path, cold_start);
                let lease_started = Instant::now();
                let lease = begin_suppression(Some(pid), prior, "LaunchAppTool.post");
                // Now safe to drop the wildcard: the targeted lease is armed.
                drop(wildcard_lease);
                tokio::time::sleep(IN_CALL_WAIT).await;

                let send_back = SendBack {
                    pid,
                    prior,
                    since: action_started,
                };
                // Belt-and-braces: if the app is in front anyway (an
                // activation the notification missed), send it straight back.
                let mut demotion_succeeded = true;
                if apps::front_pid() == Some(pid) {
                    let sent_back = send_back.send("LaunchAppTool.post");
                    demotion_succeeded = apps::front_pid() != Some(pid);
                    if !demotion_succeeded {
                        tracing::warn!(
                            target: "cu_layer::launch",
                            launched_pid = pid,
                            prior_pid = prior,
                            sent_back,
                            "launched app remained frontmost after sending it back"
                        );
                    }
                }
                self_activation_suppressed =
                    Some(apps::front_pid() != Some(pid) && demotion_succeeded);
                // The answer goes now; the watchdog keeps the app back for
                // the rest of the window (while the helper keeps running).
                tokio::spawn(send_back.keep_back(lease, lease_started + lasting));
            }
            _ => drop(wildcard_lease),
        }

        match launch_result {
            Ok(Ok(Launched { pid, app, windows })) => {
                let app_name = app.as_ref().map(|a| a.name.as_str()).unwrap_or("?");
                let bid = app
                    .as_ref()
                    .and_then(|a| a.bundle_id.as_deref())
                    .unwrap_or("?");
                let mut summary = match opened_in_background {
                    Some(opened) => {
                        let links = if link_count == 1 {
                            "the link".to_owned()
                        } else {
                            format!("{link_count} links")
                        };
                        let how = match opened.by {
                            OpenedBy::DevTools(port) => {
                                format!("through its debugging port {port}")
                            }
                            OpenedBy::AppleEvents => "by Apple Events".to_owned(),
                        };
                        let mut s = format!(
                            "{app_name} (pid {pid}) was already running: opened {links} in a new \
                             background window, {how}."
                        );
                        if windows
                            .first()
                            .is_some_and(|w| !windows_before.contains(&w.window_id))
                        {
                            s.push_str(" The new window is listed first.");
                        }
                        s
                    }
                    None => format!("Launched {app_name} (pid {pid}) in background.{port_summary}"),
                };

                if !windows.is_empty() {
                    summary.push_str("\n\nWindows:");
                    for w in &windows {
                        let title = if w.title.is_empty() {
                            "(no title)".to_owned()
                        } else {
                            format!("\"{}\"", w.title)
                        };
                        summary.push_str(&format!("\n- {title} [window_id: {}]", w.window_id));
                    }
                    summary.push_str(&format!(
                        "\n→ Call get_window_state(pid: {pid}, window_id) to inspect."
                    ));
                }

                let windows_json: Vec<Value> = windows
                    .iter()
                    .map(|w| {
                        json!({
                            "window_id": w.window_id,
                            "pid": w.pid,
                            "app_name": w.app_name,
                            "title": w.title,
                            "bounds": {
                                "x": w.bounds.x, "y": w.bounds.y,
                                "width": w.bounds.width, "height": w.bounds.height
                            },
                            "is_on_screen": w.is_on_screen,
                        })
                    })
                    .collect();

                let mut structured = json!({
                    "pid": pid,
                    "bundle_id": bid,
                    "name": app_name,
                    "windows": windows_json,
                });
                if let Some(suppressed) = self_activation_suppressed {
                    structured["self_activation_suppressed"] = Value::Bool(suppressed);
                }
                if let Some(opened) = opened_in_background {
                    structured["opened_in_background"] = match opened.by {
                        OpenedBy::DevTools(port) => json!({ "by": "debugging_port", "port": port }),
                        OpenedBy::AppleEvents => json!({ "by": "apple_events" }),
                    };
                }
                if !moved_targets.is_empty() {
                    summary.push_str(&format!(
                        "\nThe app was already running, so {} from additional_arguments \
                         opened as urls.",
                        moved_targets.join(", ")
                    ));
                    structured["opened_from_arguments"] = json!(moved_targets);
                }
                if let Some(hint) = settings_hint(bid, link_count == 0) {
                    summary.push('\n');
                    summary.push_str(hint);
                }
                ToolResult::text(summary).with_structured(structured)
            }
            Ok(Err(e)) => ToolResult::error(format!("Launch failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// Sends a launched app back behind the app that was in front.
#[derive(Clone, Copy)]
struct SendBack {
    pid: i32,
    prior: i32,
    /// When the call began: an app launched since then is the call's own,
    /// so the restore policy sends it back also right after the user's
    /// input.
    since: Instant,
}

impl SendBack {
    /// Put the prior app back in front at once, if Brigadier's restore
    /// policy allows it. `true` when it was asked to come back.
    fn send(&self, origin: &'static str) -> bool {
        let request = RestoreRequest {
            kind: RestoreKind::SelfActivation,
            activated_pid: self.pid,
            restore_to: self.prior,
            target_pid: Some(self.pid),
            since: self.since,
            origin,
        };
        if !restore_allowed(&request) {
            return false;
        }
        apps::set_front_pid(self.prior) || platform_macos::apps::activate_pid(self.prior)
    }

    /// Keep the app from the front until `until`: a suppression lease
    /// renewed before the driver ends it, and a check every tick that sends
    /// the app straight back when it is in front anyway.
    async fn keep_back(self, mut lease: SuppressionLease, until: Instant) {
        let mut renewed = Instant::now();
        let mut late_activations = 0u32;
        loop {
            let now = Instant::now();
            if now >= until {
                break;
            }
            tokio::time::sleep(TICK.min(until - now)).await;
            if renewed.elapsed() >= LEASE_RENEWAL && Instant::now() < until {
                // The fresh lease is armed before the old one ends.
                lease = begin_suppression(Some(self.pid), self.prior, "LaunchAppTool.watchdog");
                renewed = Instant::now();
            }
            if apps::front_pid() == Some(self.pid) && self.send("LaunchAppTool.watchdog") {
                late_activations += 1;
            }
        }
        drop(lease);
        if late_activations > 0 {
            tracing::info!(
                target: "cu_layer::launch",
                launched_pid = self.pid,
                prior_pid = self.prior,
                late_activations,
                "sent a launched app back after the call answered"
            );
        }
    }
}

/// How many 250 ms ticks a launched app is kept from coming to the front.
/// An app that starts from scratch activates itself once its first window
/// is ready, seconds after the launch returns (Electron apps especially), as
/// does one that is handed documents, so both get ~10.5 s; an app that was
/// already running only reacts at once.
pub fn watchdog_ticks(slow_launch_path: bool, cold_start: bool) -> u32 {
    if slow_launch_path || cold_start {
        42
    } else {
        2
    }
}

/// Whether a failed launch is tried again: before the deadline, once in any
/// case, and after that only while an earlier instance is still quitting.
pub fn retry_launch(past_deadline: bool, retried: bool, still_running: bool) -> bool {
    !past_deadline && (!retried || still_running)
}

/// Whether a running app (its bundle id and name) is the one a call names:
/// by bundle id when given, else by name.
pub fn names_app(
    app_bundle_id: Option<&str>,
    app_name: &str,
    bundle_id: Option<&str>,
    name: Option<&str>,
) -> bool {
    match bundle_id {
        Some(bundle_id) => app_bundle_id == Some(bundle_id),
        None => name == Some(app_name),
    }
}

/// Lists the window `first` first (the app's main window: the one the
/// launch opened or brought up), ahead of any panels or helper windows,
/// keeping the others in order.
pub fn put_window_first(windows: &mut [WindowInfo], first: Option<u32>) {
    if let Some(at) = windows.iter().position(|w| Some(w.window_id) == first) {
        windows[..=at].rotate_right(1);
    }
}

/// The line for an answer that opened System Settings without a pane.
pub fn settings_hint(bundle_id: &str, no_urls: bool) -> Option<&'static str> {
    (no_urls && bundle_id == SETTINGS_BUNDLE_ID).then_some(SETTINGS_HINT)
}

/// The pid's layer-0 windows, waiting up to 5 × 100 ms for the WindowServer
/// to register them after LaunchServices returns.
fn resolve_windows_for_pid(pid: i32) -> Vec<WindowInfo> {
    for attempt in 0..5 {
        let found = open_link::current_windows_for_pid(pid);
        if !found.is_empty() {
            return found;
        }
        if attempt < 4 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    vec![]
}

fn structured_launch_error(code: &str, message: String, details: Value) -> ToolResult {
    let mut payload = json!({ "error": code });
    if let Value::Object(payload) = &mut payload {
        match details {
            Value::Object(details) => payload.extend(details),
            details => {
                payload.insert("details".to_string(), details);
            }
        }
    }
    ToolResult::error(message).with_structured(payload)
}

/// Splits launch arguments into files and links to open (a link, or an
/// absolute or `~` path) and the rest (flags and their values).
pub fn split_open_targets(arguments: Vec<String>) -> (Vec<String>, Vec<String>) {
    arguments.into_iter().partition(|argument| {
        !argument.starts_with('-')
            && (argument.contains("://") || argument.starts_with('/') || argument.starts_with('~'))
    })
}

/// A structured `FILE_NOT_FOUND` error for the first local file among
/// `urls` that does not exist.
pub fn preflight_file_urls(urls: &[String]) -> Option<ToolResult> {
    for raw in urls {
        let Some(path) = local_file_target(raw) else {
            continue;
        };
        if !path.exists() {
            return Some(structured_launch_error(
                "FILE_NOT_FOUND",
                format!(
                    "Local launch_app url target does not exist: {}",
                    path.display()
                ),
                json!({
                    "url": raw,
                    "path": path.display().to_string(),
                }),
            ));
        }
    }
    None
}

/// The local file a url names (a path, `~` path or `file://` URL), or `None`
/// for a link or an app's own scheme.
pub fn local_file_target(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() {
        return Some(PathBuf::from(raw));
    }
    if let Some(rest) = raw.strip_prefix("file://") {
        let path = rest.strip_prefix("localhost").unwrap_or(rest);
        // A page anchor or query isn't part of the file's path.
        let path = path.split(['#', '?']).next().unwrap_or(path);
        return Some(expand_tilde(&percent_decode_path(path)));
    }
    let looks_like_url = raw.contains(':') && !raw.starts_with('/') && !raw.starts_with('~');
    if looks_like_url {
        return None;
    }
    Some(expand_tilde(raw))
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home);
        }
    } else if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn percent_decode_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                decoded.push((high << 4) | low);
                i += 3;
                continue;
            }
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
