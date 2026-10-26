//! brigadier-cu — Brigadier's background computer-use helper (MCP server).
//!
//! Runs as a MCP JSON-RPC 2.0 server over stdio, as the daemon behind it, or
//! as a one-shot CLI. Every entry point builds its tools through
//! `cu_layer::registry::build`.
//!
//! Extra CLI flags (consumed here, not by MCP), read by
//! `cursor_overlay::CursorConfig::from_args`:
//!   --cursor-theme <id>                   installed cursor theme
//!   --cursor-reduced-motion <auto|on|off> motion preference
//!   --no-overlay                          start with overlay disabled
//!   --glide-ms     <f64>                  glide duration override
//!   --dwell-ms     <f64>                  post-click dwell override
//!   --idle-hide-ms <f64>                  idle-hide timeout override
//!
//! ## macOS threading model
//!
//! AppKit requires the main thread. `main` spawns the server on a worker
//! thread and runs AppKit on the main thread: the cursor overlay's loop when
//! the cursor is on (it never returns, so the worker ends the process
//! itself), otherwise the plain event loop so browser Apple Events and other
//! main-queue work remain serviceable.

// A refusal is the driver's own `ToolResult`, returned as is to the agent.
#![allow(clippy::result_large_err)]

// Moved whole from the old core; a few of its items have no caller here yet.
#[allow(dead_code)]
mod activity;
mod activity_daemon;
mod agent_apps;
#[cfg(target_os = "macos")]
mod appkit;
mod bundle;
// Moved whole from the old driver crate; a few of its items have no caller
// here yet.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod capture_probe;
mod cli;
mod doctor;
mod env;
mod home;
mod host;
mod journal;
mod lifecycle;
mod mcp;
mod owner_watch;
#[cfg(target_os = "macos")]
mod process_name;
// Process facts the supervisor needs on Windows and Linux.
#[cfg(not(target_os = "macos"))]
mod process;
// Moved whole from the old driver crate; a few of its items have no caller
// here yet.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod permissions;
mod proxy;
mod responsibility;
mod scope_args;
mod serve;
mod supervisor;
mod transport;

use std::sync::Arc;

use cua_driver_core::tool::ToolRegistry;

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::from_env("BRIGADIER_CU_LOG").add_directive(tracing::Level::WARN.into()),
        )
        .init();
}

// ── Registry ─────────────────────────────────────────────────────────────

/// Build the tool registry the one way every entry point does
/// (`cu_layer::registry::build`). Exits when the driver's settings are
/// invalid: no tool could run.
pub(crate) fn build_registry(
    cursor: cursor_overlay::CursorConfig,
    compat: bool,
) -> Arc<ToolRegistry> {
    let host_bundle_id = std::env::var(cu_layer::HOST_BUNDLE_ID_ENV)
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| !id.is_empty());
    match cu_layer::registry::build(cu_layer::registry::Options {
        cursor,
        compat,
        host_bundle_id,
    }) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("brigadier-cu: the driver's settings are invalid: {error}");
            std::process::exit(78);
        }
    }
}

/// The registry for an entry point that never shows the agent cursor.
pub(crate) fn one_shot_registry(compat: bool) -> Arc<ToolRegistry> {
    build_registry(
        cursor_overlay::CursorConfig {
            enabled: false,
            ..Default::default()
        },
        compat,
    )
}

// ── macOS entry-point ─────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn run_guarded_appkit_worker(name: &str, work: impl FnOnce()) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)) {
        Ok(()) => 0,
        Err(_) => {
            eprintln!("[brigadier-cu] {name} worker panicked");
            101
        }
    }
}

/// End a worker paired with the main thread's AppKit loop. The cursor
/// overlay's loop never returns, so when it owns the main thread the worker
/// ends the process itself; otherwise it stops the plain loop and main joins.
#[cfg(target_os = "macos")]
fn end_appkit_worker(exit_code: i32, overlay_owns_main: bool) -> i32 {
    appkit::request_appkit_main_loop_stop();
    if overlay_owns_main {
        std::process::exit(exit_code);
    }
    exit_code
}

#[cfg(target_os = "macos")]
fn finish_appkit_worker(handle: std::thread::JoinHandle<i32>) {
    let exit_code = handle.join().unwrap_or(101);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

/// Run AppKit on the main thread while a worker serves: the overlay's loop
/// when it owns main (never returns), else the plain loop when a Window
/// Server is available, then join the worker.
#[cfg(target_os = "macos")]
fn run_main_thread(
    handle: std::thread::JoinHandle<i32>,
    overlay_owns_main: bool,
    has_graphic_access: bool,
) {
    if overlay_owns_main {
        platform_macos::cursor::overlay::run_on_main_thread();
    } else if has_graphic_access {
        appkit::run_appkit_main_loop();
    }
    finish_appkit_worker(handle);
}

/// Turn `--home <dir>` into the home environment variable before anything
/// reads a path. `open --args` drops the caller's environment, so the
/// daemon learns its home from argv; children it spawns inherit the env.
fn apply_home_flag() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--home") {
        if let Some(dir) = args.get(i + 1).filter(|d| !d.is_empty()) {
            // Called first thing in main, before any thread starts.
            std::env::set_var(crate::home::HOME_ENV, dir);
        }
    }
    // Brigadier's setup window, likewise.
    if args
        .iter()
        .any(|a| a == crate::lifecycle::EXTERNAL_SETUP_FLAG)
    {
        std::env::set_var(crate::lifecycle::EXTERNAL_SETUP_ENV, "1");
    }
}

#[cfg(target_os = "macos")]
fn main() {
    apply_home_flag();
    // The driver keeps its state under the helper's home. Changes the
    // environment, so it runs before logging and before any thread.
    cu_layer::configure_environment(&crate::home::home());
    init_logging();

    // Standalone-helper identity: when Brigadier spawns the bundled helper (both the
    // long-running `mcp` server and one-shot `call check_permissions` status
    // probes) it sets BRIGADIER_CU_RESPONSIBILITY_DISCLAIMED, asking this process to disclaim
    // responsibility so it becomes its own responsible process. Because the
    // executable lives inside the helper's own .app bundle, macOS then
    // attributes — and reports — its Accessibility / Screen Recording grants
    // under that bundle's identity ("Brigadier Computer Use") instead of Brigadier's.
    // Runs before command dispatch so it applies uniformly to every subcommand;
    // reexec_disclaimed_if_needed is still a no-op for an already-disclaimed
    // re-exec or a binary inside Brigadier Computer Use.app.
    if crate::bundle::is_env_truthy("BRIGADIER_CU_RESPONSIBILITY_DISCLAIMED") {
        responsibility::reexec_disclaimed_if_needed();
    }

    // ── CLI subcommand dispatch ──────────────────────────────────────────────
    // Handled before AppKit init so `list-tools` / `describe` / `call` exit
    // cleanly without starting the overlay or NSApplication.
    let command = cli::parse_command();
    match command {
        cli::Command::ListTools => {
            cli::run_list_tools(&one_shot_registry(false));
            return;
        }
        cli::Command::Describe(name) => {
            cli::run_describe(&one_shot_registry(false), &name);
            return;
        }
        cli::Command::Complain {
            text,
            tool,
            session,
        } => {
            journal::run_complain(text, tool, session);
            return;
        }
        cli::Command::Host { subcommand, args } => host::run(&subcommand, &args),
        cli::Command::Journal { subcommand, args } => {
            journal::run_journal(&subcommand, &args);
            return;
        }
        cli::Command::Call {
            tool,
            json_args,
            screenshot_out_file,
            socket,
        } => {
            let reg = one_shot_registry(false);
            if platform_macos::session::has_graphic_access() {
                // Browser automation dispatches NSAppleScript to the AppKit
                // main queue. Run the one-shot tool on a worker while main
                // pumps that queue, then terminate with the tool's exit code.
                appkit::prepare_appkit_main_loop();
                appkit::mark_appkit_main_loop_available();
                let call_handle = std::thread::Builder::new()
                    .name("cu-call".into())
                    .spawn(move || {
                        let exit_code = run_guarded_appkit_worker("call", || {
                            cli::run_call(reg, &tool, json_args, screenshot_out_file, socket);
                        });
                        end_appkit_worker(exit_code, false)
                    })
                    .expect("spawn call thread");
                run_main_thread(call_handle, false, true);
            } else {
                // Headless calls cannot target GUI browsers and must avoid
                // registering NSApplication with a missing Window Server.
                cli::run_call(reg, &tool, json_args, screenshot_out_file, socket);
            }
            return;
        }
        cli::Command::Serve {
            socket,
            no_permissions_gate,
            claude_code_compat,
            owner_pid,
        } => {
            responsibility::reexec_disclaimed_if_needed();
            let gate_opts = permissions::GateOpts::from_env_and_flag(no_permissions_gate);

            // The DAEMON is the process that performs clicks and AX presses,
            // so the agent-cursor overlay runs HERE: the builder starts it
            // when the cursor is on and a Window Server is available, and the
            // main thread then runs the overlay's loop. The compat flag is
            // the one the MCP proxy forwards (`cli::launch_daemon`).
            let cursor_cfg = cli::cursor_config();
            let cursor_enabled = cursor_cfg.enabled;
            let reg = build_registry(cursor_cfg, claude_code_compat);
            let pid_path = cli::daemon_pid_file_path(socket.as_deref());
            let sp = cli::daemon_socket_path(socket);
            let has_graphic_access = platform_macos::session::has_graphic_access();
            let overlay_owns_main = cursor_enabled && has_graphic_access;
            appkit::prepare_appkit_main_loop();

            // Bind the Unix socket FIRST, on a background thread, BEFORE
            // running the (blocking) permissions gate (#1761).
            //
            // The gate's `wait_for_grants` blocks while `ai.brigadier.computer-use`
            // is ungranted — it prompts and re-exec-loops until the user
            // grants or the deadline elapses. If serve ran after the gate,
            // the daemon's socket wouldn't appear for minutes on first
            // launch, so `permissions grant` / MCP clients launched via
            // `open -n -g -a 'Brigadier Computer Use' --args serve` (the correct-TCC-
            // attribution path) couldn't reach the daemon to even report
            // "pending". Binding the socket first makes the daemon
            // reachable within ~1s while the gate works toward the grant.
            //
            // A Unix socket + tokio accept loop has no main-thread
            // requirement, so serve runs on a background thread. The gate
            // stays on the MAIN thread: its prompt APIs
            // (`request_accessibility` / `request_screen_recording`) and
            // the NSPanel must run on main. On grant, the gate's
            // `reexec_self()` execvp's the whole daemon — the socket
            // re-binds fast on restart (run_serve unlinks the stale socket
            // file first) and stabilizes once the grant sticks.
            let serve_handle = std::thread::Builder::new()
                .name("cu-serve".into())
                .spawn(move || {
                    let exit_code = run_guarded_appkit_worker("serve", || {
                        serve::run_serve_cmd(
                            reg,
                            &sp,
                            Some(&pid_path),
                            serve::DaemonProfile::Native,
                            owner_pid,
                        );
                    });
                    end_appkit_worker(exit_code, overlay_owns_main)
                })
                .expect("spawn serve thread");

            // Socket is binding/bound now → daemon reachable while we gate.
            //
            // First-launch permissions gate (Swift PermissionsGate parity).
            // Runs on every `serve` start; no-op when both grants are
            // already active.  Honors --no-permissions-gate and
            // BRIGADIER_CU_PERMISSIONS_GATE=0 for CI / headless.
            //
            // Failures (e.g. deadline elapsed without grants) are logged
            // and the daemon continues to serve — individual tool calls
            // will then fail with the underlying TCC error, mirroring
            // Swift's "user closed the panel" fallback.
            if let Err(e) = permissions::run_if_needed(gate_opts) {
                eprintln!("[brigadier-cu] permissions gate: {e}");
                eprintln!(
                    "[brigadier-cu] continuing — tool calls touching AX or \
                           Screen Recording fail until you grant the missing TCC \
                           permissions."
                );
            }

            // Keep the main thread alive for the daemon: the overlay's loop
            // or the plain AppKit loop, both of which service browser
            // automation's main-queue dispatch. Headless daemons join and
            // never touch NSApplication.
            if has_graphic_access {
                appkit::mark_appkit_main_loop_available();
                lifecycle::start_main_loop_heartbeat();
            }
            run_main_thread(serve_handle, overlay_owns_main, has_graphic_access);
            return;
        }
        cli::Command::Stop { socket } => {
            let sp = cli::daemon_socket_path(socket);
            serve::run_stop_cmd(&sp);
            return;
        }
        cli::Command::Status { socket } => {
            let pid_path = cli::daemon_pid_file_path(socket.as_deref());
            let sp = cli::daemon_socket_path(socket);
            serve::run_status_cmd(&sp, &pid_path);
            return;
        }
        cli::Command::Recording {
            subcommand,
            args,
            socket,
        } => {
            cli::run_recording_cmd(&subcommand, &args, socket.as_deref());
            return;
        }
        cli::Command::Doctor { json } => {
            cli::run_doctor_cmd(json);
            return;
        }
        cli::Command::Diagnose => {
            cli::run_diagnose_cmd(one_shot_registry(false));
            return;
        }
        cli::Command::Permissions { subcommand, json } => {
            cli::run_permissions_cmd(one_shot_registry(false), &subcommand, json);
            return;
        }
        cli::Command::Config {
            subcommand,
            key,
            value,
            socket,
        } => {
            cli::run_config_cmd(
                one_shot_registry(false),
                subcommand.as_deref(),
                key.as_deref(),
                value.as_deref(),
                socket.as_deref(),
            );
            return;
        }
        cli::Command::Mcp {
            no_daemon_relaunch,
            socket,
            claude_code_compat,
        } => {
            // TCC sidestep: if we're a shell-spawned bare binary that
            // resolves into /Applications/Brigadier Computer Use.app, run the
            // proxy path instead of the in-process MCP server. The
            // proxy ensures a daemon is up under the bundle's TCC
            // attribution and forwards stdio MCP through its socket.
            // Issue #1525 / mirror of Swift PR #1479.
            if cli::should_use_daemon_proxy(no_daemon_relaunch) {
                if let Err(e) = cli::run_mcp_via_daemon_proxy(socket, claude_code_compat) {
                    eprintln!("brigadier-cu: {e}");
                    std::process::exit(1);
                }
                return;
            }
            // The `socket` flag is daemon-proxy-only; it has no meaning in
            // the in-process path.
            let _ = socket;
            run_in_process_mcp(claude_code_compat);
        }
    }
}

/// The in-process MCP server: the server on a worker thread, AppKit on the
/// main thread.
#[cfg(target_os = "macos")]
fn run_in_process_mcp(claude_code_compat: bool) {
    let cursor_cfg = cli::cursor_config();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        cursor_id = %cursor_cfg.cursor_id,
        overlay_enabled = cursor_cfg.enabled,
        "brigadier-cu starting (macOS)"
    );
    let cursor_enabled = cursor_cfg.enabled;
    // The builder starts the overlay before the worker exists, so the main
    // thread's overlay loop always finds it ready.
    let registry = build_registry(cursor_cfg, claude_code_compat);
    let has_graphic_access = platform_macos::session::has_graphic_access();
    let overlay_owns_main = cursor_enabled && has_graphic_access;
    appkit::prepare_appkit_main_loop();
    if has_graphic_access {
        appkit::mark_appkit_main_loop_available();
    }
    let mcp_handle = std::thread::Builder::new()
        .name("cu-mcp".into())
        .spawn(move || {
            let exit_code = run_guarded_appkit_worker("mcp", || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(async move {
                    if let Err(e) = crate::mcp::run(registry).await {
                        tracing::error!("MCP server error: {e}");
                    }
                });
            });
            end_appkit_worker(exit_code, overlay_owns_main)
        })
        .expect("spawn mcp thread");

    // Main thread: AppKit overlay or plain event loop. Browser automation
    // requires main-queue dispatch even when `--no-overlay` is set.
    run_main_thread(mcp_handle, overlay_owns_main, has_graphic_access);
}

// ── Windows and Linux entry point ─────────────────────────────────────────

/// Windows and Linux: no AppKit main thread, so every command runs here.
/// `serve` is the daemon Brigadier talks to; `mcp` serves in-process.
#[cfg(not(target_os = "macos"))]
fn main() {
    apply_home_flag();
    // The driver keeps its state under the helper's home. Changes the
    // environment, so it runs before logging and before any thread.
    cu_layer::configure_environment(&crate::home::home());
    init_logging();

    match cli::parse_command() {
        cli::Command::ListTools => cli::run_list_tools(&one_shot_registry(false)),
        cli::Command::Describe(name) => cli::run_describe(&one_shot_registry(false), &name),
        cli::Command::Complain {
            text,
            tool,
            session,
        } => journal::run_complain(text, tool, session),
        cli::Command::Host { subcommand, args } => host::run(&subcommand, &args),
        cli::Command::Journal { subcommand, args } => journal::run_journal(&subcommand, &args),
        cli::Command::Call {
            tool,
            json_args,
            screenshot_out_file,
            socket,
        } => cli::run_call(
            one_shot_registry(false),
            &tool,
            json_args,
            screenshot_out_file,
            socket,
        ),
        cli::Command::Serve {
            socket,
            no_permissions_gate,
            claude_code_compat,
            owner_pid,
        } => {
            // Windows and Linux have no permission prompts to gate on.
            let _ = no_permissions_gate;
            let reg = build_registry(cli::cursor_config(), claude_code_compat);
            let pid_path = cli::daemon_pid_file_path(socket.as_deref());
            let sp = cli::daemon_socket_path(socket);
            // run_serve_cmd builds its own runtime, so it runs on a thread of
            // its own.
            let exit_code = std::thread::Builder::new()
                .name("cu-serve".into())
                .spawn(move || {
                    serve::run_serve_cmd(
                        reg,
                        &sp,
                        Some(&pid_path),
                        serve::DaemonProfile::Native,
                        owner_pid,
                    );
                })
                .expect("spawn serve thread")
                .join()
                .map_or(101, |()| 0);
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        cli::Command::Stop { socket } => serve::run_stop_cmd(&cli::daemon_socket_path(socket)),
        cli::Command::Status { socket } => {
            let pid_path = cli::daemon_pid_file_path(socket.as_deref());
            serve::run_status_cmd(&cli::daemon_socket_path(socket), &pid_path);
        }
        cli::Command::Recording {
            subcommand,
            args,
            socket,
        } => cli::run_recording_cmd(&subcommand, &args, socket.as_deref()),
        cli::Command::Doctor { json } => cli::run_doctor_cmd(json),
        cli::Command::Diagnose => cli::run_diagnose_cmd(one_shot_registry(false)),
        cli::Command::Permissions { subcommand, json } => {
            cli::run_permissions_cmd(one_shot_registry(false), &subcommand, json)
        }
        cli::Command::Config {
            subcommand,
            key,
            value,
            socket,
        } => cli::run_config_cmd(
            one_shot_registry(false),
            subcommand.as_deref(),
            key.as_deref(),
            value.as_deref(),
            socket.as_deref(),
        ),
        cli::Command::Mcp {
            no_daemon_relaunch,
            socket,
            claude_code_compat,
        } => {
            // Brigadier's agents go through the daemon, as on macOS; a helper
            // run by hand serves in-process.
            if cli::should_use_daemon_proxy(no_daemon_relaunch) {
                if let Err(e) = cli::run_mcp_via_daemon_proxy(socket, claude_code_compat) {
                    eprintln!("brigadier-cu: {e}");
                    std::process::exit(1);
                }
                return;
            }
            let _ = socket;
            run_in_process_mcp(claude_code_compat);
        }
    }
}

/// The in-process MCP server on Windows and Linux.
#[cfg(not(target_os = "macos"))]
fn run_in_process_mcp(claude_code_compat: bool) {
    let registry = build_registry(cli::cursor_config(), claude_code_compat);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async move {
        if let Err(error) = crate::mcp::run(registry).await {
            tracing::error!("MCP server error: {error}");
        }
    });
}
