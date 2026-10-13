//! brigadier-cu — cross-platform background computer-use automation daemon.
//!
//! Runs as a MCP JSON-RPC 2.0 server over stdio.  The platform backend is
//! selected at compile time via conditional compilation.
//!
//! Extra CLI flags (consumed here, not by MCP):
//!   --cursor-icon  <path.svg|.ico|.png>   custom cursor shape
//!   --cursor-id    <id>                   multi-cursor instance id
//!   --cursor-palette <name>               named colour palette
//!   --no-overlay                          start with overlay disabled
//!   --glide-ms     <f64>                  glide duration override
//!   --dwell-ms     <f64>                  post-click dwell override
//!   --idle-hide-ms <f64>                  idle-hide timeout override
//!
//! ## macOS threading model
//!
//! AppKit requires the main thread.  On macOS the entry-point is a plain
//! `fn main()` that:
//!  1. Initialises the cursor overlay channel synchronously (so
//!     `run_on_main_thread` always finds it ready).
//!  2. Spawns a background tokio thread for the MCP server.
//!  3. Runs AppKit on the main thread. With the cursor enabled this uses the
//!     overlay loop; otherwise it uses the plain event loop so browser Apple
//!     Events and other main-queue work remain serviceable.
//!
//! On all other platforms `#[tokio::main]` is used directly.

mod activity_daemon;
mod agent_apps;
mod bundle;
mod cli;
mod doctor;
mod host;
mod journal;
mod lifecycle;
mod owner_watch;
mod proxy;
mod responsibility;
mod serve;
mod supervisor;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Set by the `Command::Mcp` arm when `--claude-code-computer-use-compat`
/// is on argv. Read by `build_registry` / `build_registry_no_cursor` to
/// pick which `screenshot` tool variant to register. Static keeps the
/// thread of dependency arrows pointed away from the platform crates —
/// they take `compat: bool` directly, but the binary crate decides what
/// to pass without making every Command variant carry the flag.
static CLAUDE_CODE_COMPAT: AtomicBool = AtomicBool::new(false);

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::from_env("BRIGADIER_CU_LOG").add_directive(tracing::Level::WARN.into()),
        )
        .init();
}

/// Wire up the experimental picture-in-picture preview window.
///
/// Called from every long-running entry point (Serve and Mcp on all
/// platforms; the `Call` arm intentionally skips PiP since the
/// per-call binaries don't keep an AppKit/event loop alive long
/// enough to be useful).
///
/// No-op when `--experimental-pip` is not on argv. On Windows / Linux
/// the factory returns "not yet implemented" — we log and continue
/// without a window so the rest of the daemon keeps working.
fn maybe_init_pip() {
    let cfg = match pip_preview::default_config_path() {
        Some(p) => pip_preview::PipConfig::from_args_and_file(&p),
        None => pip_preview::PipConfig::from_args(),
    };
    if !cfg.enabled {
        return;
    }

    // Register the platform factory. The set is idempotent so multiple
    // entry points calling this in the same process is safe.
    #[cfg(target_os = "macos")]
    pip_preview::set_pip_backend_factory(Box::new(platform_macos::pip::MacosPipBackendFactory));

    match pip_preview::start_pip(&cfg) {
        Ok(backend) => {
            // Bridge: when the tool dispatcher in cu-core wants
            // to push a frame, forward to the live backend handle.
            // We move the Box into a static Mutex<Option<...>> so the
            // closure can re-borrow on every call without taking
            // ownership of the trait object.
            use std::sync::Mutex as StdMutex;
            static BACKEND: std::sync::OnceLock<
                StdMutex<Option<Box<dyn pip_preview::PipBackend>>>,
            > = std::sync::OnceLock::new();
            let _ = BACKEND.set(StdMutex::new(Some(backend)));
            cu_core::pip_hook::set_pip_push_fn(|frame| {
                if let Some(slot) = BACKEND.get() {
                    if let Some(b) = slot.lock().unwrap().as_ref() {
                        b.push_frame(pip_preview::PipFrame {
                            png_bytes: frame.png_bytes,
                            action_label: frame.action_label,
                            timestamp_ms: frame.timestamp_ms,
                        });
                    }
                }
            });
            eprintln!("PiP preview enabled (experimental).");
        }
        Err(e) => {
            eprintln!("⚗️  PiP preview requested but unavailable: {e}");
        }
    }
}

// ── Registry helpers (macOS) ─────────────────────────────────────────────

/// Build the macOS tool registry.
#[cfg(target_os = "macos")]
fn build_macos_registry() -> cu_core::tool::ToolRegistry {
    platform_macos::register_tools()
}

#[cfg(target_os = "macos")]
fn build_macos_registry_with_compat(claude_compat: bool) -> cu_core::tool::ToolRegistry {
    platform_macos::register_tools_with_compat(claude_compat)
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

#[cfg(target_os = "macos")]
fn finish_appkit_worker(handle: std::thread::JoinHandle<i32>) {
    let exit_code = handle.join().unwrap_or(101);
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod appkit_worker_tests {
    use super::run_guarded_appkit_worker;

    #[test]
    fn worker_panic_becomes_failure_status_instead_of_stranding_main_loop() {
        assert_eq!(run_guarded_appkit_worker("test", || {}), 0);
        platform_macos::pip::prepare_appkit_main_loop();
        platform_macos::pip::mark_appkit_main_loop_available();
        let worker = std::thread::spawn(|| {
            let status = run_guarded_appkit_worker("test", || panic!("worker failed"));
            platform_macos::pip::request_appkit_main_loop_stop();
            status
        });
        assert_eq!(worker.join().unwrap(), 101);
        assert!(platform_macos::pip::appkit_main_loop_stop_requested());
        assert!(!platform_macos::pip::appkit_main_loop_available());
    }
}

/// Turn `--home <dir>` into the home environment variable before anything
/// reads a path. `open --args` drops the caller's environment, so the
/// daemon learns its home from argv; children it spawns inherit the env.
fn apply_home_flag() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--home") {
        if let Some(dir) = args.get(i + 1).filter(|d| !d.is_empty()) {
            // Called first thing in main, before any thread starts.
            std::env::set_var(pip_preview::home::HOME_ENV, dir);
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
    init_logging();

    // Standalone-helper identity: when Brigadier spawns the bundled helper (both the
    // long-running `mcp` server and one-shot `call check_permissions` status
    // probes) it sets BRIGADIER_CU_RESPONSIBILITY_DISCLAIMED, asking this process to disclaim
    // responsibility so it becomes its own responsible process. Because the
    // executable lives inside the helper's own .app bundle, macOS then
    // attributes — and reports — its Accessibility / Screen Recording grants
    // under that bundle's identity ("Brigadier Computer Use") instead of Brigadier's.
    // Runs before command dispatch so it applies uniformly to every subcommand;
    // reexec_disclaimed_if_needed is still a no-op for embedded mode, an
    // already-disclaimed re-exec, or a binary inside Brigadier Computer Use.app.
    if crate::bundle::is_env_truthy("BRIGADIER_CU_RESPONSIBILITY_DISCLAIMED") {
        responsibility::reexec_disclaimed_if_needed();
    }

    // ── CLI subcommand dispatch ──────────────────────────────────────────────
    // Handled before AppKit init so `list-tools` / `describe` / `call` exit
    // cleanly without starting the overlay or NSApplication.
    let command = cli::parse_command();
    match command {
        cli::Command::ListTools => {
            let reg = Arc::new(build_macos_registry());
            cli::run_list_tools(&reg);
            return;
        }
        cli::Command::Describe(name) => {
            let reg = Arc::new(build_macos_registry());
            cli::run_describe(&reg, &name);
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
            // Register callbacks (needed if the tool does screenshots/recording).
            cu_core::recording::set_screenshot_fn(|window_id, pid| {
                if let Some(wid) = window_id {
                    platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
                } else if let Some(p) = pid {
                    platform_macos::windows::resolve_main_window_id(p as i32)
                        .ok()
                        .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
                } else {
                    platform_macos::capture::screenshot_display_bytes().ok()
                }
            });
            cu_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
                platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
            });
            cu_core::recording::set_ax_snapshot_fn(|window_id, pid| {
                platform_macos::recording_hooks::app_state_json_for(window_id, pid)
            });
            cu_core::recording::set_element_bounds_fn(|wid, pid, idx| {
                platform_macos::recording_hooks::element_window_local_xy(wid, pid, idx)
            });
            cu_core::video::set_video_backend_factory(Box::new(
                platform_macos::video_sckit::SckitVideoBackendFactory,
            ));
            let reg = Arc::new(build_macos_registry());
            reg.init_self_weak();
            if platform_macos::session::has_graphic_access() {
                // Browser automation dispatches NSAppleScript to the AppKit
                // main queue. Run the one-shot tool on a worker while main
                // pumps that queue, then terminate with the tool's exit code.
                platform_macos::pip::prepare_appkit_main_loop();
                platform_macos::pip::mark_appkit_main_loop_available();
                let call_handle = std::thread::Builder::new()
                    .name("cu-call".into())
                    .spawn(move || {
                        let exit_code = run_guarded_appkit_worker("call", || {
                            cli::run_call(reg, &tool, json_args, screenshot_out_file, socket);
                        });
                        platform_macos::pip::request_appkit_main_loop_stop();
                        exit_code
                    })
                    .expect("spawn call thread");
                platform_macos::pip::run_appkit_main_loop();
                finish_appkit_worker(call_handle);
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
            let gate_opts =
                platform_macos::permissions::GateOpts::from_env_and_flag(no_permissions_gate);
            cu_core::recording::set_screenshot_fn(|window_id, pid| {
                if let Some(wid) = window_id {
                    platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
                } else if let Some(p) = pid {
                    platform_macos::windows::resolve_main_window_id(p as i32)
                        .ok()
                        .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
                } else {
                    platform_macos::capture::screenshot_display_bytes().ok()
                }
            });
            cu_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
                platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
            });
            cu_core::recording::set_ax_snapshot_fn(|window_id, pid| {
                platform_macos::recording_hooks::app_state_json_for(window_id, pid)
            });
            cu_core::recording::set_element_bounds_fn(|wid, pid, idx| {
                platform_macos::recording_hooks::element_window_local_xy(wid, pid, idx)
            });
            cu_core::video::set_video_backend_factory(Box::new(
                platform_macos::video_sckit::SckitVideoBackendFactory,
            ));
            maybe_init_pip();

            // Agent-cursor overlay. The DAEMON is the process that actually
            // performs clicks / AX presses, so the overlay NSWindow + render
            // loop must run HERE — not only in the in-process `mcp` arm. In the
            // daemon-proxy setup (`mcp` relaunches `open -n -g … serve` and
            // proxies to it), the proxy never renders and, before this, neither
            // did the daemon — so every cursor command was a silent no-op and
            // the agent cursor never appeared. Init the channel before spawning
            // the serve thread so `run_on_main_thread()` always finds it ready
            // (mirrors the Mcp arm).
            let cursor_cfg = cli::cursor_config();
            if cursor_cfg.enabled {
                platform_macos::cursor::overlay::init(cursor_cfg.clone());
            }

            // Honour the compat flag forwarded by the MCP proxy
            // (cli::launch_daemon passes `serve
            // --claude-code-computer-use-compat`). The Serve arm is the daemon
            // the proxy talks to, so without this the proxy path always served
            // the full screenshot tool regardless of the client's request.
            let reg = Arc::new(build_macos_registry_with_compat(claude_code_compat));
            reg.init_self_weak();
            let pid_path = cli::daemon_pid_file_path(socket.as_deref());
            let sp = cli::daemon_socket_path(socket);
            let has_graphic_access = platform_macos::session::has_graphic_access();
            platform_macos::pip::prepare_appkit_main_loop();

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
                    platform_macos::pip::request_appkit_main_loop_stop();
                    exit_code
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
            if let Err(e) = platform_macos::permissions::run_if_needed(gate_opts) {
                eprintln!("[brigadier-cu] permissions gate: {e}");
                eprintln!(
                    "[brigadier-cu] continuing — tool calls touching AX or \
                           Screen Recording fail until you grant the missing TCC \
                           permissions."
                );
            }

            // Keep the main thread alive for the daemon.
            //
            // The cursor loop also services PiP and browser automation main-
            // queue dispatch. Without a cursor, keep a plain AppKit loop alive
            // whenever a Window Server is available. Headless daemons retain
            // the old join behavior and never touch NSApplication.
            if has_graphic_access {
                platform_macos::pip::mark_appkit_main_loop_available();
                lifecycle::start_main_loop_heartbeat();
            }
            if cursor_cfg.enabled && has_graphic_access {
                // Render the agent-cursor overlay: park the main thread in the
                // AppKit run loop so the overlay NSWindow draws. `run_on_main_thread`
                // self-guards on `has_graphic_access()` and returns immediately
                // when the daemon has no Window Server session — fall through to
                // join so the daemon still serves headless. The serve thread runs
                // on its background thread regardless.
                platform_macos::cursor::overlay::run_on_main_thread();
            } else if has_graphic_access {
                platform_macos::pip::run_appkit_main_loop();
            }
            finish_appkit_worker(serve_handle);
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
            let reg = Arc::new(build_macos_registry());
            cli::run_diagnose_cmd(reg);
            return;
        }
        cli::Command::Permissions { subcommand, json } => {
            let reg = Arc::new(build_macos_registry());
            cli::run_permissions_cmd(reg, &subcommand, json);
            return;
        }
        cli::Command::Config {
            subcommand,
            key,
            value,
            socket,
        } => {
            let reg = Arc::new(build_macos_registry());
            reg.init_self_weak();
            cli::run_config_cmd(
                reg,
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
            CLAUDE_CODE_COMPAT.store(claude_code_compat, Ordering::SeqCst);
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
            // Fall through to the in-process MCP server below. The
            // `socket` flag is daemon-proxy-only; it has no meaning
            // in the in-process path, so we drop it on the floor.
            let _ = socket;
        }
    }

    let cursor_cfg = cli::cursor_config();

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        cursor_id = %cursor_cfg.cursor_id,
        overlay_enabled = cursor_cfg.enabled,
        has_custom_icon = cursor_cfg.shape.is_some(),
        "brigadier-cu starting (macOS)"
    );

    let enabled = cursor_cfg.enabled;

    // Initialise overlay channel synchronously BEFORE spawning background
    // thread.  This eliminates a race where run_on_main_thread() could be
    // called before init() and find an empty channel.
    if enabled {
        platform_macos::cursor::overlay::init(cursor_cfg.clone());
    }

    // Spawn tokio + MCP server on a background thread so the main thread
    // is free for AppKit.
    // Register screenshot callback for recording (post-action screenshots).
    cu_core::recording::set_screenshot_fn(|window_id, pid| {
        if let Some(wid) = window_id {
            platform_macos::capture::screenshot_window_bytes(wid as u32).ok()
        } else if let Some(p) = pid {
            platform_macos::windows::resolve_main_window_id(p as i32)
                .ok()
                .and_then(|wid| platform_macos::capture::screenshot_window_bytes(wid).ok())
        } else {
            platform_macos::capture::screenshot_display_bytes().ok()
        }
    });

    // Register click-marker callback for recording (click.png with red crosshair).
    cu_core::recording::set_click_marker_fn(|png_bytes, cx, cy| {
        platform_macos::capture::crosshair_png_bytes(png_bytes, cx, cy).ok()
    });
    cu_core::recording::set_ax_snapshot_fn(|window_id, pid| {
        platform_macos::recording_hooks::app_state_json_for(window_id, pid)
    });
    cu_core::recording::set_element_bounds_fn(|wid, pid, idx| {
        platform_macos::recording_hooks::element_window_local_xy(wid, pid, idx)
    });
    cu_core::video::set_video_backend_factory(Box::new(
        platform_macos::video_sckit::SckitVideoBackendFactory,
    ));
    maybe_init_pip();

    let has_graphic_access = platform_macos::session::has_graphic_access();
    platform_macos::pip::prepare_appkit_main_loop();
    if has_graphic_access {
        platform_macos::pip::mark_appkit_main_loop_available();
    }
    let mcp_handle = std::thread::Builder::new()
        .name("cu-mcp".into())
        .spawn(move || {
            let exit_code = run_guarded_appkit_worker("mcp", || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                let compat = CLAUDE_CODE_COMPAT.load(Ordering::SeqCst);
                rt.block_on(async move {
                    // Register tools; overlay init has already happened above.
                    let registry = Arc::new(build_macos_registry_with_compat(compat));
                    // Wire up replay tool's back-reference to the registry.
                    registry.init_self_weak();
                    let result = cu_core::server::run(registry.clone()).await;
                    if let Err(e) = result {
                        tracing::error!("MCP server error: {e}");
                    }
                    registry.remove_state_file();
                });
            });
            platform_macos::pip::request_appkit_main_loop_stop();
            exit_code
        })
        .expect("spawn mcp thread");

    // Main thread: AppKit overlay or plain event loop. Browser automation
    // requires main-queue dispatch even when `--no-overlay` is set.
    if enabled && has_graphic_access {
        platform_macos::cursor::overlay::run_on_main_thread();
    } else if has_graphic_access {
        platform_macos::pip::run_appkit_main_loop();
    }
    finish_appkit_worker(mcp_handle);
}
