//! CLI subcommand parsing and execution.
//!
//! Subcommand dispatch (mirrors the Swift brigadier-cu CLI):
//!
//!   brigadier-cu                              → mcp server (default)
//!   brigadier-cu mcp                          → mcp server (explicit)
//!   brigadier-cu list-tools                   → print all tool names + descriptions
//!   brigadier-cu describe <tool>              → print tool schema
//!   brigadier-cu call <tool> [json-args]      → invoke tool, print result
//!   brigadier-cu <tool> [json-args]           → shorthand for call (snake_case names)
//!
//! Cursor-overlay flags (--cursor-id, --no-overlay, etc.) are consumed by
//! `CursorConfig::from_args()` and are ignored here.

use cu_core::{protocol::Content, tool::ToolRegistry};
use std::process;

/// Which CLI command was requested.
pub enum Command {
    Mcp {
        /// Force in-process MCP execution — skip the TCC auto-relaunch
        /// path that would spawn a daemon via `open -n -g -a "Brigadier Computer Use"
        /// --args serve` and proxy stdio MCP requests through its Unix
        /// socket. Useful when the calling context already has the right
        /// TCC grants (Brigadier Computer Use.app launched us directly), or when
        /// diagnosing in-process failures. Also toggleable via
        /// `BRIGADIER_CU_MCP_NO_RELAUNCH=1`.
        no_daemon_relaunch: bool,
        /// Override the daemon Unix socket path used by the proxy
        /// fallback. Defaults to `serve::default_socket_path()`.
        socket: Option<String>,
        /// `--claude-code-computer-use-compat`: register the compat
        /// `screenshot` tool (window-scoped, JPEG @ 85%, pid + window_id
        /// both required) instead of the full-featured one. Used when
        /// the MCP server is wired up as `cu-computer-use` in Claude
        /// Code, where this is the documented best-practice install.
        claude_code_compat: bool,
    },
    ListTools,
    Describe(String),
    Call {
        tool: String,
        json_args: Option<serde_json::Value>,
        screenshot_out_file: Option<String>,
        /// Override the daemon socket/pipe path used by the in-process
        /// forwarding fallback (matches `--socket` semantics for `serve` /
        /// `status` / `stop`). Defaults to `serve::default_socket_path()`
        /// when None — i.e. `brigadier-cu call X` looks for the user's
        /// default-path daemon. Surfaced to make integration tests able
        /// to spin up a tempfile-socketed daemon and route calls
        /// through it.
        socket: Option<String>,
    },
    Serve {
        socket: Option<String>,
        /// True when `--no-permissions-gate` is on argv.  The env-var
        /// `BRIGADIER_CU_PERMISSIONS_GATE=0` short-circuits the gate too
        /// (checked inside the gate itself), so the flag is only one of
        /// two opt-out signals.
        no_permissions_gate: bool,
        /// True when `--claude-code-computer-use-compat` is on argv. The MCP
        /// proxy forwards this flag to the daemon it auto-launches (see
        /// `launch_daemon_and_wait`) so the proxy path registers the compat
        /// `screenshot` surface, not just the in-process path. Without it the
        /// flag was a no-op for `brigadier-cu mcp --claude-code-computer-use-compat`,
        /// which always routes through the proxy on an installed bundle.
        claude_code_compat: bool,
        /// `--owner-pid <pid>`: exit (removing the socket) when this process
        /// exits. A host app that launches serve through LaunchServices
        /// passes its own pid so a crash does not orphan the daemon.
        owner_pid: Option<i32>,
    },
    Stop {
        socket: Option<String>,
    },
    Status {
        socket: Option<String>,
    },
    Recording {
        subcommand: String,
        args: Vec<String>,
        socket: Option<String>,
    },
    Doctor {
        json: bool,
    },
    Diagnose,
    /// `brigadier-cu permissions status|grant [--json]`: report profile-bound
    /// daemon TCC status or open correctly-attributed BrigadierCu onboarding.
    Permissions {
        subcommand: String,
        json: bool,
    },
    Config {
        /// `show` | `get` | `set` | `reset` (None → show)
        subcommand: Option<String>,
        /// key arg for `get`/`set`
        key: Option<String>,
        /// value arg for `set`
        value: Option<String>,
        socket: Option<String>,
    },
    /// `brigadier-cu complain <text>`: an agent records a problem it hit in
    /// the opt-in local reliability journal (see `journal.rs`).
    Complain {
        text: String,
        tool: Option<String>,
        session: Option<String>,
    },
    /// `brigadier-cu journal {status|enable|disable|path|tail|summary}`.
    Journal {
        subcommand: String,
        args: Vec<String>,
    },
    /// `brigadier-cu host …`: Brigadier's own requests (see `host.rs`).
    Host {
        subcommand: String,
        args: Vec<String>,
    },
}

/// Flags whose next token is a value (not a subcommand).
/// We skip both the flag and its value when scanning for the subcommand.
const VALUE_FLAGS: &[&str] = &[
    "--cursor-icon",
    "--cursor-id",
    "--cursor-palette",
    "--cursor-shape",
    "--glide-ms",
    "--dwell-ms",
    "--idle-hide-ms",
    "--screenshot-out-file",
    "--client",
    "--socket",
    "--pid-file",
    "--type",
    "--host-bundle-id",
    "--owner-pid",
    "--home",
    // Experimental PiP preview — value flag for the optional geometry
    // override (--experimental-pip itself is a bare flag and doesn't
    // need to be listed here).
    "--experimental-pip-geometry",
];

/// Parse the cursor-overlay flags from argv.
pub fn cursor_config() -> cursor_overlay::CursorConfig {
    let args: Vec<String> = std::env::args().skip(1).collect();
    cursor_overlay::CursorConfig::parse(&args)
}

/// Parse the first non-flag positional argument from argv to determine which
/// subcommand to run.  Cursor-overlay flags are consumed by `CursorConfig`
/// independently; we only care about the first non-`--` arg here.
pub fn parse_command() -> Command {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Handle --version / -V before any other parsing so they are never
    // silently stripped as "bare flags" and swallowed by MCP mode.
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("brigadier-cu {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "brigadier-cu {} — cross-platform computer-use automation driver",
            env!("CARGO_PKG_VERSION")
        );
        println!("Usage: brigadier-cu [SUBCOMMAND] [OPTIONS]");
        println!("Subcommands: mcp, list-tools, describe, call, serve, stop, status, config, recording, doctor, diagnose, permissions");
        println!();
        println!("permissions options (macOS):");
        println!("  brigadier-cu permissions status   Report Accessibility + Screen Recording status. Read-only (no prompt).");
        println!("                                  Answers via a running daemon, so the result carries the BrigadierCu");
        println!("                                  identity (ai.brigadier.computer-use). If no daemon is running it reports");
        println!("                                  `unknown` rather than your terminal's grants. Add --json for the payload.");
        println!("  brigadier-cu permissions grant    Open or reopen BrigadierCu permission onboarding in a running daemon.");
        println!("                                  Launches the app if needed, then refreshes every live profile so");
        println!("                                  status reflects ai.brigadier.computer-use instead of your terminal.");
        println!();
        println!("serve options:");
        println!("  --owner-pid <pid>       Exit 0 and remove the socket when <pid> exits (for example the");
        println!(
            "                          host app crashes). Exits at once if <pid> is already dead."
        );
        println!(
            "                          macOS: kqueue NOTE_EXIT. Linux: pidfd. Windows: ignored."
        );
        println!();
        println!("mcp options (macOS):");
        println!("  --no-daemon-relaunch    Stay in-process; skip auto-launching the BrigadierCu daemon.");
        println!("                          Also: BRIGADIER_CU_MCP_NO_RELAUNCH=1");
        println!("  --embedded              Run embedded inside a host app (also: BRIGADIER_CU_EMBEDDED=1).");
        println!("                          Inherits the host's TCC grants; never prompts or relaunches.");
        println!("                          See Skills/brigadier-cu/EMBEDDING.md.");
        println!(
            "  --host-bundle-id <id>   Advisory host bundle id label for check_permissions output."
        );
        println!(
            "  --socket <path>         Override the daemon UDS path used by the proxy fallback."
        );
        println!("  --claude-code-computer-use-compat");
        println!("                          Select the Claude Code computer-use compat surface.");
        println!(
            "                          Now forwarded to the proxy-launched daemon (was a no-op"
        );
        println!(
            "                          on the proxy path — the path you actually run — because"
        );
        println!("                          the daemon hardcoded compat=false). Note: the compat");
        println!(
            "                          screenshot tool itself was removed in #1692, so the flag"
        );
        println!(
            "                          has no tool-surface effect today; the wiring is in place"
        );
        println!("                          for any future compat-gated tool.");
        println!();
        println!("agent cursor overlay (serve / mcp only — needs the daemon UI runloop):");
        println!("  Embedded stdio processes (BRIGADIER_CU_EMBEDDED=1) get an automatic default");
        println!(
            "  cursor keyed by BRIGADIER_CU_DEFAULT_SESSION (else embedded-<pid>), so anonymous"
        );
        println!("  calls show a cursor with no session arg. Non-embedded callers must pass an");
        println!("  explicit `session` to get a cursor. The cursor shows where the agent acts");
        println!("  without moving the real pointer and is removed when the session ends. A pure accessibility (AX)");
        println!(
            "  action snaps the cursor with a brief pulse on its first action instead of a long"
        );
        println!("  glide, so it can be easy to miss — do a pixel click or move_cursor first");
        println!("  for a visibly gliding demo. These flags tune the overlay on `serve`/`mcp`:");
        println!("  --no-overlay            Disable the cursor overlay entirely for this daemon.");
        println!(
            "  --cursor-id <id>        Name the default cursor instance (default: 'default')."
        );
        println!("  --cursor-icon <path>    Use a custom PNG / SVG / ICO cursor asset.");
        println!(
            "  --cursor-shape <name>   Built-in silhouette: {} ('teardrop' is the default —",
            cursor_overlay::BuiltinShape::names_help()
        );
        println!(
            "                          embedded cursor-up SVG; 'arrow' is the procedural gradient"
        );
        println!(
            "                          diamond; 'sky' is the gray kite cursor; 'Brigadier' is"
        );
        println!("                          Lawrence's fixed upright branded Sky kite).");
        println!("                          Same vocabulary as MCP `cursor_icon`.");
        println!("  --cursor-palette <name> Pick a built-in colour palette for the cursor.");
        println!(
            "  (These are no-ops for one-shot CLI calls like `brigadier-cu call` — the overlay"
        );
        println!("   needs the long-lived AppKit runloop that only `serve` / `mcp` keep alive.)");
        println!();
        println!("reliability journal (opt-in, local only):");
        println!("  brigadier-cu complain <text> [--tool NAME] [--session ID]");
        println!("                                Record a problem you hit with brigadier-cu. Agents: use this");
        println!("                                when a tool fails, hangs, or behaves wrongly. Reads stdin");
        println!("                                when no text is given. No-op unless the journal is enabled.");
        println!("  brigadier-cu journal status|enable|disable|path");
        println!("  brigadier-cu journal tail [-n N] [--errors] [--follow] [--json]");
        println!("  brigadier-cu journal summary [--hours H] [--json]");
        println!("                                The journal records every proxied tool call (tool, outcome,");
        println!("                                duration, error text; never argument values) to");
        println!("                                ~/Library/Logs/brigadier-cu/journal.jsonl. Off by default.");
        println!();
        println!("doctor options:");
        println!("  --json                  Emit the probe report as JSON for scripting.");
        println!();
        println!("experimental options (default: off):");
        println!("  --experimental-pip          Show a small always-on-top window with the latest");
        println!(
            "                              post-action screenshot + a 1-line label. macOS only"
        );
        println!(
            "                              today; Win/Linux print a not-yet-implemented notice."
        );
        println!(
            "  --experimental-pip-geometry WxH[+X+Y]   Override window size (and optional top-left"
        );
        println!("                                          origin). Defaults to 480x360 in the top-right");
        println!("                                          corner of the main display.");
        std::process::exit(0);
    }

    // Collect named flag values we care about.
    let screenshot_out_file = flag_value(&args, "--screenshot-out-file");
    let socket = flag_value(&args, "--socket");

    // `--embedded` / `--host-bundle-id` export to the environment rather
    // than threading through `Command`: all consumers read
    // `crate::env::embedded_mode()` and children inherit the mode.
    if args.iter().any(|a| a == "--embedded") {
        std::env::set_var(crate::env::EMBEDDED_ENV, "1");
    }
    if let Some(id) = flag_value(&args, "--host-bundle-id") {
        std::env::set_var(crate::env::HOST_BUNDLE_ID_ENV, id);
    }

    // Strip cursor-overlay flags (and their values) to expose the subcommand.
    let mut positionals: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if VALUE_FLAGS.contains(&a) {
            i += 2; // skip flag + value
        } else if a.starts_with('-') {
            i += 1; // skip bare flag
        } else {
            positionals.push(a);
            i += 1;
        }
    }

    let no_daemon_relaunch = args.iter().any(|a| a == "--no-daemon-relaunch");
    let claude_code_compat = args
        .iter()
        .any(|a| a == "--claude-code-computer-use-compat");

    let mut pos = positionals.into_iter();
    match pos.next() {
        None => {
            // Bare `brigadier-cu` defaults to MCP, which reads JSON-RPC from
            // stdin forever. From a terminal that looks like a hang. If
            // stdin is a TTY (i.e. interactive shell, no client piping
            // stdio), surface a hint and exit. Piped / redirected stdin —
            // the normal MCP client case — falls through to MCP mode.
            // Explicit `brigadier-cu mcp` bypasses the check entirely.
            use std::io::IsTerminal as _;
            if std::io::stdin().is_terminal() {
                eprintln!("brigadier-cu: bare invocation defaults to the MCP server, which reads");
                eprintln!("JSON-RPC from stdin. From a terminal that looks like a hang.");
                eprintln!();
                eprintln!("You probably meant one of:");
                eprintln!("  brigadier-cu list-tools                           # available tools");
                eprintln!("  brigadier-cu status                               # check the daemon");
                eprintln!("  brigadier-cu --help                               # everything else");
                eprintln!();
                eprintln!("To run the MCP server explicitly (and pipe JSON-RPC by hand):");
                eprintln!("  brigadier-cu mcp");
                std::process::exit(0);
            }
            Command::Mcp {
                no_daemon_relaunch,
                socket: socket.clone(),
                claude_code_compat,
            }
        }
        Some("mcp") => Command::Mcp {
            no_daemon_relaunch,
            socket: socket.clone(),
            claude_code_compat,
        },
        Some("complain") | Some("report-issue") => {
            let tool = flag_value(&args, "--tool");
            let session = flag_value(&args, "--session");
            let mut words: Vec<String> = Vec::new();
            let mut skip_next = false;
            for a in args
                .iter()
                .skip_while(|a| *a != "complain" && *a != "report-issue")
                .skip(1)
            {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if a == "--tool" || a == "--session" {
                    skip_next = true;
                    continue;
                }
                if a.starts_with("--tool=") || a.starts_with("--session=") {
                    continue;
                }
                words.push(a.clone());
            }
            let mut text = words.join(" ");
            if text.trim().is_empty() {
                let mut stdin = String::new();
                if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin);
                }
                text = stdin;
            }
            Command::Complain {
                text,
                tool,
                session,
            }
        }
        Some("journal") => {
            let subcommand = pos.next().unwrap_or("status").to_string();
            let rest: Vec<String> = args
                .iter()
                .skip_while(|a| *a != "journal")
                .skip(2)
                .cloned()
                .collect();
            Command::Journal {
                subcommand,
                args: rest,
            }
        }
        Some("host") => {
            let subcommand = pos.next().unwrap_or("probe").to_string();
            let rest: Vec<String> = args
                .iter()
                .skip_while(|a| *a != "host")
                .skip(2)
                .cloned()
                .collect();
            Command::Host {
                subcommand,
                args: rest,
            }
        }
        Some("list-tools") => Command::ListTools,
        Some("serve") => Command::Serve {
            socket,
            // Bare flag — present anywhere on argv counts as "skip the gate".
            no_permissions_gate: args.iter().any(|a| a == "--no-permissions-gate"),
            claude_code_compat,
            owner_pid: serve_owner_pid_or_exit(&args),
        },
        Some("stop") => Command::Stop { socket },
        Some("status") => Command::Status { socket },
        Some("recording") => {
            let subcommand = pos.next().unwrap_or("status").to_string();
            let rest: Vec<String> = pos.map(str::to_owned).collect();
            Command::Recording {
                subcommand,
                args: rest,
                socket,
            }
        }
        Some("doctor") => {
            // `--json` switches to machine-readable output for scripting.
            // Bare flag — no value, position-independent.
            let json = args.iter().any(|a| a == "--json");
            Command::Doctor { json }
        }
        Some("diagnose") => Command::Diagnose,
        Some("permissions") => {
            let subcommand = pos.next().unwrap_or("status").to_string();
            let json = args.iter().any(|a| a == "--json");
            Command::Permissions { subcommand, json }
        }
        Some("config") => {
            let subcommand = pos.next().map(str::to_owned);
            let key = pos.next().map(str::to_owned);
            let value = pos.next().map(str::to_owned);
            Command::Config {
                subcommand,
                key,
                value,
                socket,
            }
        }
        Some("describe") => {
            let name = pos.next().unwrap_or("").to_string();
            Command::Describe(name)
        }
        Some("call") => {
            let tool = pos.next().unwrap_or("").to_string();
            // Differentiate "no positional arg" (fall back to stdin) from
            // "positional arg given but didn't parse as JSON" (surface the
            // error instead of silently falling back to stdin and letting
            // the tool's required-field validator emit a misleading
            // "missing field X" later). See #1637.
            //
            // The common cause of an unparseable positional arg is
            // PowerShell 5.1's native-command-arg quote-stripping on
            // multi-field JSON — `'{"a":1,"b":2}'` arrives as `{a:1,b:2}`
            // which serde_json rejects. The error message points users at
            // the stdin-pipe workaround.
            let json_args = match pos.next() {
                Some(s) => match serde_json::from_str(s) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        eprintln!(
                            "error: positional JSON arg to 'brigadier-cu call' did not parse: {e}"
                        );
                        eprintln!("       received: {s}");
                        eprintln!();
                        eprintln!("hint: PowerShell 5.1 strips quotes around JSON field names in");
                        eprintln!("      multi-field args. Pipe the JSON via stdin instead:");
                        eprintln!(
                            "        '{{\"pid\":1234,\"window_id\":5678}}' | brigadier-cu call {}",
                            tool
                        );
                        eprintln!();
                        eprintln!("      Or use PowerShell 7+ (pwsh) which preserves the quotes.");
                        process::exit(2);
                    }
                },
                None => read_stdin_json(),
            };
            Command::Call {
                tool,
                json_args,
                screenshot_out_file,
                socket: socket.clone(),
            }
        }
        Some(first) => {
            // Implicit call: unrecognised first positional → treat as tool name.
            // Same parse-error handling as the explicit `call` branch above. See #1637.
            let tool = first.to_string();
            let json_args = match pos.next() {
                Some(s) => match serde_json::from_str(s) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        eprintln!("error: positional JSON arg to 'brigadier-cu {tool}' did not parse: {e}");
                        eprintln!("       received: {s}");
                        eprintln!();
                        eprintln!("hint: PowerShell 5.1 strips quotes around JSON field names in");
                        eprintln!("      multi-field args. Pipe the JSON via stdin instead:");
                        eprintln!(
                            "        '{{\"pid\":1234,\"window_id\":5678}}' | brigadier-cu {}",
                            tool
                        );
                        eprintln!();
                        eprintln!("      Or use PowerShell 7+ (pwsh) which preserves the quotes.");
                        process::exit(2);
                    }
                },
                None => read_stdin_json(),
            };
            Command::Call {
                tool,
                json_args,
                screenshot_out_file,
                socket: socket.clone(),
            }
        }
    }
}

pub(crate) use crate::owner_watch::parse_owner_pid;

/// Read `serve --owner-pid <pid>`. A missing or invalid value is a usage
/// error (exit 2), never a silent fallback to an unowned daemon.
fn serve_owner_pid_or_exit(args: &[String]) -> Option<i32> {
    let present = args
        .iter()
        .any(|a| a == "--owner-pid" || a.starts_with("--owner-pid="));
    if !present {
        return None;
    }
    let raw = flag_value(args, "--owner-pid").unwrap_or_default();
    match parse_owner_pid(&raw) {
        Ok(pid) => Some(pid),
        Err(message) => {
            eprintln!("brigadier-cu serve: {message}");
            std::process::exit(2);
        }
    }
}

/// Return the value of `--flag value` from argv, or `None`.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().cloned();
        }
        // Also handle --flag=value form.
        if let Some(rest) = a.strip_prefix(flag) {
            if let Some(val) = rest.strip_prefix('=') {
                return Some(val.to_owned());
            }
        }
    }
    None
}

/// Print all tools in the registry, one per line: `name: first sentence`.
pub fn run_list_tools(registry: &ToolRegistry) {
    // Sort alphabetically by name to match Swift's
    // `ListToolsCommand.run()` `tools.sorted(by: { $0.name < $1.name })`.
    let mut entries: Vec<(&str, &cu_core::tool::ToolDef)> = registry.iter_defs().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, def) in entries {
        let summary = first_sentence(&def.description);
        if summary.is_empty() {
            println!("{name}");
        } else {
            println!("{name}: {summary}");
        }
    }
}

/// Print a tool's full description and JSON input schema.
/// Exits 64 (EX_USAGE) if the tool is unknown.
pub fn run_describe(registry: &ToolRegistry, name: &str) {
    match registry.get_def(name) {
        None => {
            eprintln!("Unknown tool: {name}");
            eprintln!("Available tools:");
            // Sort alphabetically to match Swift's `printUnknownTool`
            // (`registry.allTools.map(\.name).sorted()`).
            let mut names: Vec<&str> = registry.tool_names().collect();
            names.sort();
            for n in names {
                eprintln!("  {n}");
            }
            process::exit(64);
        }
        Some(def) => {
            println!("name: {}", def.name);
            if !def.description.is_empty() {
                print!("\ndescription:\n{}", def.description);
                if !def.description.ends_with('\n') {
                    println!();
                }
            }
            print!("\ninput_schema:\n");
            let pretty = serde_json::to_string_pretty(&def.input_schema)
                .unwrap_or_else(|_| def.input_schema.to_string());
            println!("{pretty}");
        }
    }
}

/// Decide whether `mcp` should auto-launch a daemon and proxy MCP
/// requests through its Unix socket instead of running in-process.
///
/// Mirrors Swift `MCPCommand.shouldUseDaemonProxy` in spirit:
/// the trigger is "shell-spawned bare binary that resolves into an
/// installed `Brigadier Computer Use.app` bundle, with a non-launchd parent".
/// When any of those conditions fails — explicit opt-out, dev-mode
/// `cargo run` invocation, already-relaunched-via-launchd — we stay
/// in-process. The proxy path is purely additive.
///
/// `false` on non-macOS targets: TCC is a macOS-only concern and
/// there's no `open -a` equivalent on Linux / Windows.
#[cfg(target_os = "macos")]
pub fn should_use_daemon_proxy(no_daemon_relaunch: bool) -> bool {
    use crate::bundle::{
        is_env_truthy, is_executable_inside_brigadier_cu_app, parent_is_not_launchd,
    };
    // Embedded mode stays in-process: relaunching via `open -a BrigadierCu`
    // would leave the host's TCC responsibility chain and could prompt
    // for ai.brigadier.computer-use.
    if crate::env::embedded_mode() {
        return false;
    }
    if no_daemon_relaunch {
        return false;
    }
    if is_env_truthy("BRIGADIER_CU_MCP_NO_RELAUNCH") {
        return false;
    }
    // Hidden test/escape hook: force proxy mode without requiring the
    // executable to live inside Brigadier Computer Use.app. Used by the
    // integration test (which spawns a daemon manually) and by users
    // who've wrapped the binary in a custom bundle. Skips the
    // launch_daemon_and_wait `open -a` step too — caller is expected
    // to have a daemon already running on the chosen socket.
    // Brigadier names a specific helper app to launch as the daemon (see
    // launch_daemon_and_wait). Take the proxy path so that app — launched via
    // LaunchServices under its own TCC identity — performs the TCC-gated work,
    // instead of this in-process server running under the host app's identity.
    if std::env::var_os("BRIGADIER_CU_DAEMON_APP").is_some() {
        return true;
    }
    if !is_executable_inside_brigadier_cu_app() {
        // Raw `cargo run` / dev binary — no installed bundle to land
        // in, so relaunching would fail. Stay in-process.
        return false;
    }
    if !parent_is_not_launchd() {
        // ppid == 1 — already running as the LaunchServices-spawned
        // daemon. TCC context is already correct.
        return false;
    }
    true
}

/// Resolve the daemon endpoint for an MCP/serve invocation.
pub fn daemon_socket_path(explicit_socket: Option<String>) -> String {
    explicit_socket.unwrap_or_else(crate::serve::default_socket_path)
}

pub fn daemon_pid_file_path(explicit_socket: Option<&str>) -> String {
    if let Some(socket) = explicit_socket {
        let mut path = std::path::PathBuf::from(socket);
        path.set_extension("pid");
        return path.to_string_lossy().into_owned();
    }
    crate::serve::default_pid_file_path()
}

/// Launch the daemon through LaunchServices with
/// `open -n -g -a <helper app> --args serve --home <home>` and return once
/// `open` has handed it over. A LaunchServices-launched app is its own
/// responsible process, so macOS attributes Accessibility and Screen
/// Recording to the helper bundle ("Brigadier Computer Use"), not to the
/// agent or terminal that started the shim. `open` drops the caller's
/// environment, which is why the home directory travels as an argument.
#[cfg(target_os = "macos")]
pub fn launch_daemon(socket_path: &str, claude_code_compat: bool) -> anyhow::Result<()> {
    use std::process::{Command as Cmd, Stdio};

    // Brigadier sets BRIGADIER_CU_DAEMON_APP to the placed helper app.
    let daemon_app = std::env::var("BRIGADIER_CU_DAEMON_APP")
        .unwrap_or_else(|_| "Brigadier Computer Use".to_string());
    let home = crate::home::home();
    let home = home.to_string_lossy();
    let mut open_args: Vec<&str> = vec![
        // `-n` forces a new instance so a running copy cannot swallow
        // `--args serve`; `-g` keeps it in the background.
        "-n",
        "-g",
        "-a",
        daemon_app.as_str(),
        "--args",
        "serve",
        "--home",
        &home,
    ];
    if socket_path != crate::serve::default_socket_path() {
        open_args.push("--socket");
        open_args.push(socket_path);
    }
    if claude_code_compat {
        open_args.push("--claude-code-computer-use-compat");
    }
    // Brigadier runs setup in its own window: no panel of the helper's own.
    if crate::bundle::is_env_truthy(crate::lifecycle::EXTERNAL_SETUP_ENV) {
        open_args.push("--no-permissions-gate");
        open_args.push(crate::lifecycle::EXTERNAL_SETUP_FLAG);
    }
    let status = Cmd::new("/usr/bin/open")
        .args(&open_args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| anyhow::anyhow!("failed to run /usr/bin/open: {e}"))?;
    if !status.success() {
        anyhow::bail!(
            "could not open {daemon_app} (open exited {:?})",
            status.code()
        );
    }
    Ok(())
}

/// Run the MCP proxy path: enter `crate::proxy::run_proxy` against the daemon
/// socket, allowing its macOS implementation to start or wait for the daemon
/// lazily. Builds its own tokio runtime — same shape as the other `run_*`
/// helpers in this file that own their event loop.
pub fn run_mcp_via_daemon_proxy(
    socket: Option<String>,
    claude_code_compat: bool,
) -> anyhow::Result<()> {
    // Windows: prefer the uiAccess'd worker pipe over the regular daemon pipe
    // when both are running, so MCP tool calls land in a process that can
    // bypass UIPI for UWP apps. The protocol on both pipes is identical so
    // the proxy doesn't need to know which one it's talking to. See #1602.
    let socket_path = if let Some(s) = socket {
        s
    } else {
        crate::serve::default_socket_path()
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(crate::proxy::run_proxy(
        socket_path,
        claude_code_compat,
        crate::serve::DaemonProfile::Native,
    ))
}

/// Invoke a tool, forwarding to a running daemon if one is reachable;
/// otherwise runs in-process. Prints result to stdout on success, error
/// to stderr on failure. Exits 1 if the tool returned an error result.
/// When `screenshot_out_file` is provided, image content is written there
/// instead of emitted as base64 on stdout.
///
/// `socket` — override the daemon socket path (from --socket flag).
pub fn run_call(
    registry: std::sync::Arc<ToolRegistry>,
    tool: &str,
    json_args: Option<serde_json::Value>,
    screenshot_out_file: Option<String>,
    socket_override: Option<String>,
) {
    // Daemon forwarding: if a daemon is listening, proxy the request
    // through it so AppStateEngine's element_index cache is shared.
    //
    // On Windows, prefer the uiAccess-elevated worker (brigadier-cu-uia.exe) when
    // present — it runs at UIAccess integrity and bypasses UIPI for UWP apps
    // like Calculator / modern Notepad / Settings. The regular daemon at
    // `\\.\pipe\brigadier-cu` is Medium integrity and gets ERROR_ACCESS_DENIED on
    // SendInput into AppContainer'd processes. See #1602.
    //
    // When `socket_override` is Some (i.e. caller passed `--socket <path>`),
    // route directly to that path and skip the platform default + uia worker
    // search. Used by integration tests to drive a tempfile-socketed daemon.
    let socket_path = if let Some(s) = socket_override {
        s
    } else {
        #[cfg(not(target_os = "windows"))]
        {
            crate::serve::default_socket_path()
        }
    };
    // macOS: `check_permissions` with prompt:true raises a TCC dialog. Run
    // in-process from a terminal, that dialog attributes to the *terminal*
    // (LaunchServices' "responsible" process), not to ai.brigadier.computer-use —
    // so the grant lands on the wrong app and never sticks for the driver.
    // When we're a bundle CLI spawned from a terminal (should_use_daemon_proxy)
    // and there's no daemon to route through, DON'T raise the mis-attributed
    // prompt: degrade to report-only and tell the user the one launch that
    // grants correctly (`open … BrigadierCu --args serve`, which raises the
    // dialog as BrigadierCu and waits for the grant). We deliberately do NOT
    // auto-spawn that daemon here — a `call` shouldn't leave a background
    // daemon behind, and the first-launch gate can lag socket creation.
    #[cfg(target_os = "macos")]
    let json_args = {
        let mut effective = json_args;
        let wants_prompt = effective
            .as_ref()
            .and_then(|v| v.get("prompt"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true); // check_permissions defaults prompt:true
        if tool == "check_permissions"
            && wants_prompt
            && !crate::serve::is_daemon_listening(&socket_path)
            && should_use_daemon_proxy(false)
        {
            eprintln!(
                "brigadier-cu: reporting permission status only. A prompt raised from \
                 this terminal would attribute to the terminal, not BrigadierCu, so the \
                 grant wouldn't apply to the driver. To grant correctly, launch the \
                 driver as its own app:\n  open -n -g -a 'Brigadier Computer Use' --args serve\n\
                 then approve the BrigadierCu dialog in System Settings."
            );
            effective = Some(serde_json::json!({ "prompt": false }));
        }
        effective
    };
    if crate::serve::is_daemon_listening(&socket_path) {
        let mut args_for_daemon = json_args
            .clone()
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
        crate::scope_args::scope_target_args(
            &mut args_for_daemon,
            &cu_core::target_policy::allowed_from_env(),
        );
        let req = crate::serve::DaemonRequest {
            method: "call".into(),
            name: Some(tool.to_owned()),
            args: Some(args_for_daemon),
            // CLI one-shot is its own ephemeral, anonymous/global session.
            session_id: None,
        };
        match crate::serve::send_request(&socket_path, &req) {
            Ok(resp) => {
                if resp.ok {
                    if let Some(result) = resp.result {
                        // Walk the content array once: pick up any Image
                        // payloads (either to write to --screenshot-out-file
                        // or to merge into structuredContent below).
                        let mut printed = false;
                        let mut image_b64: Option<(String, String)> = None;
                        if let Some(content) = result.get("content").and_then(|v| v.as_array()) {
                            for item in content {
                                if item.get("type").and_then(|v| v.as_str()) == Some("image") {
                                    let b64 = item
                                        .get("data")
                                        .and_then(|v| v.as_str())
                                        .map(str::to_owned);
                                    let mime = item
                                        .get("mimeType")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("image/png")
                                        .to_owned();
                                    if let Some(b64) = b64 {
                                        if let Some(ref path) = screenshot_out_file {
                                            use base64::Engine as _;
                                            match base64::engine::general_purpose::STANDARD
                                                .decode(&b64)
                                            {
                                                Ok(bytes) => {
                                                    if let Err(e) = std::fs::write(path, &bytes) {
                                                        eprintln!("--screenshot-out-file: failed to write {path}: {e}");
                                                    }
                                                }
                                                Err(e) => {
                                                    eprintln!("--screenshot-out-file: base64 decode failed: {e}");
                                                }
                                            }
                                        } else {
                                            // Stash for the structuredContent merge below.
                                            image_b64 = Some((b64, mime));
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(sc) = result.get("structuredContent") {
                            // Merge image data into the structured payload
                            // (matches in-process behaviour at the bottom of
                            // this fn) so `brigadier-cu call screenshot` over
                            // the daemon socket still emits
                            // `screenshot_png_b64`. Previously this path
                            // dropped the image entirely when no
                            // --screenshot-out-file was given.
                            let mut obj = sc.clone();
                            if let Some((b64, mime)) = image_b64 {
                                if let serde_json::Value::Object(ref mut map) = obj {
                                    map.insert(
                                        "screenshot_png_b64".into(),
                                        serde_json::Value::String(b64),
                                    );
                                    map.insert(
                                        "screenshot_mime_type".into(),
                                        serde_json::Value::String(mime),
                                    );
                                }
                            }
                            let pretty = serde_json::to_string_pretty(&obj)
                                .unwrap_or_else(|_| obj.to_string());
                            println!("{pretty}");
                            printed = true;
                        }
                        if !printed {
                            if let Some(content) = result.get("content").and_then(|v| v.as_array())
                            {
                                for item in content {
                                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                                        if let Some(text) =
                                            item.get("text").and_then(|v| v.as_str())
                                        {
                                            println!("{text}");
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return;
                } else {
                    if let Some(err) = resp.error {
                        eprintln!("{err}");
                    }
                    process::exit(resp.exit_code.unwrap_or(1));
                }
            }
            Err(e) => {
                // Daemon became unreachable mid-call — fall through to in-process.
                // Promoted from `tracing::debug!` to `eprintln!` so callers see
                // the degradation: in-process execution gets a FRESH ToolState,
                // which means state-dependent tools (`click`, `type_text`,
                // `set_value` — anything that reads the element_index cache)
                // will fail with "Element N not in cache" even when a prior
                // `get_window_state` populated the daemon's cache, because the
                // daemon's cache and the in-process cache are different.
                eprintln!(
                    "[brigadier-cu] WARNING: daemon proxy to {socket_path} failed ({e}); \
                     running '{tool}' in-process. State-dependent tools may misbehave."
                );
            }
        }
    }
    if registry.get_def(tool).is_none() {
        eprintln!("Unknown tool: {tool}");
        eprintln!("Run `brigadier-cu list-tools` to see available tools.");
        process::exit(64);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let mut args = json_args.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    crate::scope_args::scope_target_args(&mut args, &cu_core::target_policy::allowed_from_env());
    let tool_name = tool.to_string();
    let out_path = screenshot_out_file;
    let is_error = rt.block_on(async move {
        let result = registry.invoke(&tool_name, args).await;
        let is_err = result.is_error.unwrap_or(false);

        // Emit content.
        let mut has_printed = false;
        let mut image_b64: Option<(String, String)> = None; // (base64, mime)
        for item in &result.content {
            match item {
                Content::Text { text, .. } => {
                    if is_err {
                        eprintln!("{text}");
                    } else {
                        // Only print text when there is no structuredContent
                        // (structuredContent path prints below).
                        if result.structured_content.is_none() {
                            println!("{text}");
                            has_printed = true;
                        }
                    }
                }
                Content::Image {
                    data, mime_type, ..
                } => {
                    image_b64 = Some((data.clone(), mime_type.clone()));
                }
            }
        }

        // If --screenshot-out-file was provided, write the image there
        // and suppress it from the JSON output (same as Swift reference).
        if let Some(ref path) = out_path {
            if let Some((b64, _mime)) = image_b64.take() {
                use base64::Engine as _;
                match base64::engine::general_purpose::STANDARD.decode(&b64) {
                    Ok(bytes) => {
                        if let Err(e) = std::fs::write(path, &bytes) {
                            eprintln!("--screenshot-out-file: failed to write {path}: {e}");
                        }
                    }
                    Err(e) => {
                        eprintln!("--screenshot-out-file: base64 decode failed: {e}");
                    }
                }
            } else {
                eprintln!(
                    "--screenshot-out-file: no image content in tool response; file not written"
                );
            }
        }

        // If there's structuredContent, print it as JSON (with image merged in if no out_path).
        if let Some(sc) = &result.structured_content {
            if !is_err {
                let mut obj = sc.clone();
                if out_path.is_none() {
                    if let Some((b64, mime)) = image_b64 {
                        if let serde_json::Value::Object(ref mut map) = obj {
                            map.insert("screenshot_png_b64".into(), serde_json::Value::String(b64));
                            map.insert(
                                "screenshot_mime_type".into(),
                                serde_json::Value::String(mime),
                            );
                        }
                    }
                }
                let pretty = serde_json::to_string_pretty(&obj).unwrap_or_else(|_| obj.to_string());
                println!("{pretty}");
                has_printed = true;
            }
        }

        let _ = has_printed;
        is_err
    });

    if is_error {
        process::exit(1);
    }
}

/// `brigadier-cu recording <start|stop|status>` — wrapper around
/// `start_recording` / `stop_recording` / `get_recording_state` tools
/// on the running daemon.
///
/// Requires a running daemon (`brigadier-cu serve`) because recording
/// state lives in-process.
pub fn run_recording_cmd(subcommand: &str, args: &[String], socket: Option<&str>) {
    // `render` is pure file-to-file work that doesn't need the daemon;
    // dispatch it before the daemon-running check so it works without
    // a running `brigadier-cu serve`.
    if subcommand == "render" {
        run_recording_render(args);
        return;
    }

    let socket_path = socket
        .map(str::to_owned)
        .unwrap_or_else(crate::serve::default_socket_path);

    if !crate::serve::is_daemon_listening(&socket_path) {
        eprintln!(
            "Brigadier Computer Use daemon is not running.\n\
             Start it first with: brigadier-cu serve"
        );
        process::exit(1);
    }

    match subcommand {
        "start" => {
            let output_dir = match args.first() {
                Some(d) => {
                    // Expand ~ manually.
                    if d.starts_with('~') {
                        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
                        d.replacen('~', &home, 1)
                    } else {
                        d.clone()
                    }
                }
                None => {
                    eprintln!("Usage: brigadier-cu recording start <output-dir>");
                    process::exit(64);
                }
            };

            // Create the directory if it doesn't exist.
            if let Err(e) = std::fs::create_dir_all(&output_dir) {
                eprintln!("Failed to create output directory {output_dir}: {e}");
                process::exit(1);
            }

            let req = crate::serve::DaemonRequest {
                method: "call".into(),
                name: Some("start_recording".into()),
                args: Some(serde_json::json!({
                    "output_dir": output_dir
                })),
                // CLI `recording start` is anonymous — the recording is owned by
                // nobody, so only an unconditional stop (CLI / manual) reaps it.
                session_id: None,
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => {
                    println!("Recording started → {output_dir}");
                    // Query state to show next_turn.
                    let state_req = crate::serve::DaemonRequest {
                        method: "call".into(),
                        name: Some("get_recording_state".into()),
                        args: Some(serde_json::json!({})),
                        session_id: None,
                    };
                    if let Ok(sr) = crate::serve::send_request(&socket_path, &state_req) {
                        if let Some(result) = sr.result {
                            let sc = result
                                .get("structuredContent")
                                .or_else(|| result.get("structured_content"));
                            if let Some(next_turn) =
                                sc.and_then(|s| s.get("next_turn")).and_then(|v| v.as_u64())
                            {
                                println!("Next turn: {next_turn:05}");
                            }
                        }
                    }
                }
                Ok(resp) => {
                    if let Some(e) = resp.error {
                        eprintln!("{e}");
                    }
                    process::exit(1);
                }
                Err(e) => {
                    eprintln!("recording start: {e}");
                    process::exit(1);
                }
            }
        }

        "stop" => {
            let req = crate::serve::DaemonRequest {
                method: "call".into(),
                name: Some("stop_recording".into()),
                args: Some(serde_json::json!({})),
                session_id: None,
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => println!("Recording stopped."),
                Ok(resp) => {
                    if let Some(e) = resp.error {
                        eprintln!("{e}");
                    }
                    process::exit(1);
                }
                Err(e) => {
                    eprintln!("recording stop: {e}");
                    process::exit(1);
                }
            }
        }

        "status" | "" => {
            let req = crate::serve::DaemonRequest {
                method: "call".into(),
                name: Some("get_recording_state".into()),
                args: Some(serde_json::json!({})),
                session_id: None,
            };
            match crate::serve::send_request(&socket_path, &req) {
                Ok(resp) if resp.ok => {
                    if let Some(result) = resp.result {
                        let sc = result
                            .get("structuredContent")
                            .or_else(|| result.get("structured_content"))
                            .cloned()
                            .unwrap_or(serde_json::json!({}));
                        let enabled = sc.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                        let out_dir = sc
                            .get("output_dir")
                            .and_then(|v| v.as_str())
                            .unwrap_or("(none)");
                        let next_turn = sc.get("next_turn").and_then(|v| v.as_u64()).unwrap_or(0);
                        println!(
                            "Recording: {}",
                            if enabled { "enabled" } else { "disabled" }
                        );
                        if enabled {
                            println!("  output_dir: {out_dir}");
                            println!("  next_turn:  {next_turn:05}");
                        }
                    }
                }
                Ok(resp) => {
                    if let Some(e) = resp.error {
                        eprintln!("{e}");
                    }
                    process::exit(1);
                }
                Err(e) => {
                    eprintln!("recording status: {e}");
                    process::exit(1);
                }
            }
        }

        other => {
            eprintln!("Unknown recording subcommand '{other}'. Valid: start <dir>, stop, status, render <dir> --output <out.mp4>");
            process::exit(64);
        }
    }
}

/// `brigadier-cu recording render <input-dir> <out.mp4> [--no-zoom] [--scale N]`
/// Pure file-to-file work — does NOT go through the daemon.
///
/// Note on flag parsing: the global brigadier-cu CLI parser strips
/// recognised flags before this subcommand sees them, leaving only
/// positionals. So instead of `--output <out>` (which the parser
/// would consume and lose) we accept the output path as the second
/// positional. `--no-zoom` and `--scale N` survive because their
/// values are inline (no separate value token).
fn run_recording_render(args: &[String]) {
    // First positional = input dir, second positional = output mp4.
    let positionals: Vec<&String> = args.iter().filter(|s| !s.starts_with("--")).collect();
    let input_dir = match positionals.first() {
        Some(s) if !s.is_empty() => std::path::PathBuf::from(s),
        _ => {
            eprintln!("Usage: brigadier-cu recording render <input-dir> <out.mp4> [--no-zoom] [--scale N]");
            process::exit(64);
        }
    };
    let output_path = match positionals.get(1) {
        Some(s) if !s.is_empty() => std::path::PathBuf::from(s),
        _ => {
            eprintln!("Usage: brigadier-cu recording render <input-dir> <out.mp4> [--no-zoom] [--scale N]");
            eprintln!("(second positional argument is the output path)");
            process::exit(64);
        }
    };
    let mut no_zoom = false;
    let mut scale: f64 = 2.0;
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--no-zoom" => no_zoom = true,
            "--scale" => {
                if let Some(v) = iter.next() {
                    if let Ok(f) = v.parse::<f64>() {
                        scale = f;
                    }
                }
            }
            _ => {}
        }
    }

    let opts = cu_core::recording_render::RenderOptions {
        no_zoom,
        default_scale: scale,
    };
    println!(
        "Rendering {} -> {}{}",
        input_dir.display(),
        output_path.display(),
        if no_zoom { " (no-zoom baseline)" } else { "" }
    );
    match cu_core::recording_render::render(&input_dir, &output_path, &opts) {
        Ok(res) => {
            println!("✅ Wrote {}", res.output_path.display());
            println!("   input_duration_ms: {:.0}", res.input_duration_ms);
            println!("   zoom_region_count: {}", res.zoom_region_count);
        }
        Err(e) => {
            eprintln!("Render failed: {e}");
            process::exit(1);
        }
    }
}

/// `brigadier-cu permissions status|grant`.
pub fn run_permissions_cmd(_registry: std::sync::Arc<ToolRegistry>, subcommand: &str, json: bool) {
    match subcommand {
        "status" => run_permissions_status(json),
        "grant" => run_permissions_grant(),
        other => {
            eprintln!("unknown permissions subcommand '{other}'. Valid: status, grant.");
            process::exit(2);
        }
    }
}

#[derive(Clone)]
struct PermissionDaemonEndpoint {
    profile: crate::serve::DaemonProfile,
    label: &'static str,
    socket: String,
}

#[derive(Clone)]
struct PermissionDaemonState {
    endpoint: PermissionDaemonEndpoint,
    payload: serde_json::Value,
}

fn permission_daemon_endpoints() -> Vec<PermissionDaemonEndpoint> {
    vec![PermissionDaemonEndpoint {
        profile: crate::serve::DaemonProfile::Native,
        label: "Native daemon",
        socket: crate::serve::default_socket_path(),
    }]
}

fn query_permission_daemon(
    endpoint: &PermissionDaemonEndpoint,
) -> anyhow::Result<Option<PermissionDaemonState>> {
    if !crate::serve::is_daemon_listening(&endpoint.socket) {
        return Ok(None);
    }
    let request = crate::serve::DaemonRequest {
        method: "permissions_status".into(),
        name: None,
        args: None,
        session_id: None,
    };
    let response = match crate::serve::send_request(&endpoint.socket, &request) {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if !response.ok {
        anyhow::bail!(
            "{} rejected its permission status request: {}",
            endpoint.label,
            response
                .error
                .unwrap_or_else(|| "unknown daemon error".to_owned())
        );
    }
    let payload = response
        .result
        .ok_or_else(|| anyhow::anyhow!("{} returned no permission status", endpoint.label))?;
    let reported_profile = payload.get("profile").and_then(|value| value.as_str());
    if reported_profile != Some(endpoint.profile.as_str()) {
        anyhow::bail!(
            "{} on {} reported profile {:?}, expected {}",
            endpoint.label,
            endpoint.socket,
            reported_profile,
            endpoint.profile
        );
    }
    let attribution = payload
        .get("source")
        .and_then(|source| source.get("attribution"))
        .and_then(|value| value.as_str());
    if attribution != Some("driver-daemon") {
        anyhow::bail!(
            "{} did not return daemon-attributed permission state",
            endpoint.label
        );
    }
    Ok(Some(PermissionDaemonState {
        endpoint: endpoint.clone(),
        payload,
    }))
}

fn discover_permission_daemons() -> anyhow::Result<Vec<PermissionDaemonState>> {
    permission_daemon_endpoints()
        .iter()
        .map(query_permission_daemon)
        .filter_map(|result| match result {
            Ok(Some(state)) => Some(Ok(state)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

fn wait_for_permission_daemon(
    endpoint: &PermissionDaemonEndpoint,
    timeout: std::time::Duration,
) -> anyhow::Result<PermissionDaemonState> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(state) = query_permission_daemon(endpoint)? {
            return Ok(state);
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "{} did not return during permission onboarding",
                endpoint.label
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn permission_bool(state: &PermissionDaemonState, key: &str) -> bool {
    state
        .payload
        .get(key)
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn permission_instance(state: &PermissionDaemonState) -> Option<&str> {
    state
        .payload
        .get("source")
        .and_then(|source| source.get("instance"))
        .and_then(|value| value.as_str())
}

fn permission_all_granted(state: &PermissionDaemonState) -> bool {
    permission_bool(state, "accessibility") && permission_bool(state, "screen_recording")
}

/// Report permission state from every live daemon profile. A terminal process
/// is never trusted to answer for the app bundle's TCC identity.
fn run_permissions_status(json: bool) {
    let states = match discover_permission_daemons() {
        Ok(states) => states,
        Err(error) => {
            if json {
                let payload = serde_json::json!({
                    "daemon_running": true,
                    "status": "unknown",
                    "profiles": {},
                    "reason": error.to_string(),
                });
                println!("{}", serde_json::to_string_pretty(&payload).unwrap());
            } else {
                eprintln!("Could not read daemon permission state: {error}");
            }
            return;
        }
    };
    if states.is_empty() {
        if json {
            let payload = serde_json::json!({
                "daemon_running": false,
                "status": "unknown",
                "profiles": {},
                "reason": "no BrigadierCu daemon profile is available under the app bundle's own TCC identity",
            });
            println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        } else {
            println!("Accessibility:    ❓ unknown");
            println!("Screen Recording: ❓ unknown");
            println!(
                "No native or Codex Computer Use daemon is available to report its own TCC state."
            );
            println!("  Run `brigadier-cu permissions grant` to launch onboarding.");
        }
        return;
    }

    if json {
        let profiles = states
            .iter()
            .fold(serde_json::Map::new(), |mut map, state| {
                map.insert(
                    state.endpoint.profile.as_str().to_owned(),
                    state.payload.clone(),
                );
                map
            });
        let payload = serde_json::json!({
            "daemon_running": true,
            "status": "known",
            "accessibility": states
                .iter()
                .all(|state| permission_bool(state, "accessibility")),
            "screen_recording": states
                .iter()
                .all(|state| permission_bool(state, "screen_recording")),
            "all_live_profiles_granted": states.iter().all(permission_all_granted),
            "source": { "attribution": "driver-daemon" },
            "profiles": profiles,
        });
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        return;
    }

    for (index, state) in states.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("{} ({})", state.endpoint.label, state.endpoint.profile);
        for line in permission_status_lines(&state.payload) {
            println!("  {line}");
        }
    }
    if states.iter().any(|state| !permission_all_granted(state)) {
        println!("\n  Run `brigadier-cu permissions grant` to continue onboarding.");
    }
}

fn permission_status_lines(structured: &serde_json::Value) -> Vec<String> {
    let b = |k: &str| structured.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let ax = b("accessibility");
    let sr = b("screen_recording");
    let cap = structured
        .get("screen_recording_capturable")
        .and_then(|value| value.as_bool());
    let attribution = structured
        .get("source")
        .and_then(|s| s.get("attribution"))
        .and_then(|v| v.as_str())
        .unwrap_or("driver-daemon");

    let mut lines = vec![
        format!(
            "Accessibility:    {}",
            if ax { "✅ granted" } else { "❌ not granted" }
        ),
        format!(
            "Screen Recording: {}",
            if sr { "✅ granted" } else { "❌ not granted" }
        ),
    ];
    let capture_error = structured
        .get("screen_recording_capture_error")
        .filter(|value| value.is_object());
    match (cap, capture_error) {
        (_, Some(error)) => lines.push(format!(
            "Live capture probe: ⏳ {} ({}, {} ms). {} The grants above are unchanged; do not re-grant.",
            error.get("code").and_then(|v| v.as_str()).unwrap_or("capture_timeout"),
            error.get("operation").and_then(|v| v.as_str()).unwrap_or("capture"),
            error.get("elapsed_ms").and_then(|v| v.as_u64()).unwrap_or(0),
            error.get("hint").and_then(|v| v.as_str()).unwrap_or(""),
        )),
        (Some(false), None) if sr => lines.push(
            "  ⚠️  preflight reports granted, but a live capture probe failed — the grant likely belongs to another process, not this one."
                .to_owned(),
        ),
        (None, None) => lines.push(
            "Live capture probe: ❓ not performed (silent status check)".to_owned(),
        ),
        _ => {}
    }
    lines.push(format!("Source: {attribution}"));
    lines
}

#[cfg(target_os = "macos")]
fn send_permission_control(
    endpoint: &PermissionDaemonEndpoint,
    method: &str,
) -> anyhow::Result<serde_json::Value> {
    let request = crate::serve::DaemonRequest {
        method: method.to_owned(),
        name: None,
        args: None,
        session_id: None,
    };
    let response = crate::serve::send_request(&endpoint.socket, &request)?;
    if !response.ok {
        anyhow::bail!(
            "{} rejected {method}: {}",
            endpoint.label,
            response
                .error
                .unwrap_or_else(|| "unknown daemon error".to_owned())
        );
    }
    let payload = response.result.unwrap_or_else(|| serde_json::json!({}));
    if payload.get("profile").and_then(|value| value.as_str()) != Some(endpoint.profile.as_str()) {
        anyhow::bail!(
            "{} returned the wrong daemon profile for {method}",
            endpoint.label
        );
    }
    Ok(payload)
}

#[cfg(target_os = "macos")]
fn permission_onboarding_dismissed(
    state: &PermissionDaemonState,
    instance_changed: bool,
    panel_completion_baseline: u64,
) -> bool {
    let recovery = state
        .payload
        .get("recovery")
        .and_then(|value| value.get("state"))
        .and_then(|value| value.as_str());
    let panel_visible = state
        .payload
        .get("panel")
        .and_then(|value| value.get("visible"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let panel_outcome = state
        .payload
        .get("panel")
        .and_then(|value| value.get("last_outcome"))
        .and_then(|value| value.as_str());
    let panel_completion = state
        .payload
        .get("panel")
        .and_then(|value| value.get("completion"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    (!panel_visible && recovery == Some("dismissed"))
        || (!instance_changed
            && panel_completion > panel_completion_baseline
            && !panel_visible
            && panel_outcome == Some("dismissed"))
        || (instance_changed
            && panel_completion > 0
            && !panel_visible
            && panel_outcome == Some("dismissed"))
}

#[cfg(target_os = "macos")]
fn wait_for_fresh_permission_state(
    endpoint: &PermissionDaemonEndpoint,
    previous_instance: &str,
    watch_dismissal: bool,
    panel_completion_baseline: u64,
) -> anyhow::Result<PermissionDaemonState> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    loop {
        if let Some(state) = query_permission_daemon(endpoint)? {
            let instance_changed = permission_instance(&state)
                .map(|instance| instance != previous_instance)
                .unwrap_or(false);
            if instance_changed && permission_all_granted(&state) {
                return Ok(state);
            }
            if watch_dismissal
                && permission_onboarding_dismissed(
                    &state,
                    instance_changed,
                    panel_completion_baseline,
                )
            {
                anyhow::bail!("permission onboarding was closed before both grants were enabled");
            }
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!(
                "timed out waiting for {} to verify both grants",
                endpoint.label
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

#[cfg(target_os = "macos")]
fn refresh_permission_daemon(
    state: &PermissionDaemonState,
) -> anyhow::Result<PermissionDaemonState> {
    let instance = permission_instance(state)
        .ok_or_else(|| anyhow::anyhow!("{} returned no process instance", state.endpoint.label))?
        .to_owned();
    send_permission_control(&state.endpoint, "permissions_refresh")?;
    wait_for_fresh_permission_state(&state.endpoint, &instance, false, 0)
}

/// Reopen onboarding inside a live daemon, then refresh every daemon profile
/// so each one observes TCC state from a fresh process image. The private
/// control methods do not alter either profile's MCP tool roster.
fn run_permissions_grant() {
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = run_permissions_grant_macos() {
            eprintln!("Permission onboarding failed: {error}");
            process::exit(1);
        }
    }
}

#[cfg(target_os = "macos")]
fn run_permissions_grant_macos() -> anyhow::Result<()> {
    let mut live = discover_permission_daemons()?;
    if live.is_empty() {
        let native = permission_daemon_endpoints()
            .into_iter()
            .find(|endpoint| endpoint.profile == crate::serve::DaemonProfile::Native)
            .expect("native permission endpoint");
        println!("Launching BrigadierCu permission onboarding.");
        crate::supervisor::ensure(&native.socket, false)?;
        let ready_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(state) = query_permission_daemon(&native)? {
                live.push(state);
                break;
            }
            if std::time::Instant::now() >= ready_deadline {
                anyhow::bail!("native daemon started but did not return permission state");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    } else {
        println!("Using the running BrigadierCu daemon to open permission onboarding.");
    }

    if live.iter().any(permission_all_granted) {
        for state in live.iter().filter(|state| !permission_all_granted(state)) {
            refresh_permission_daemon(state)?;
        }
        println!("BrigadierCu has Accessibility and Screen Recording for every live profile.");
        return Ok(());
    }

    let presenter = live
        .iter()
        .find(|state| state.endpoint.profile == crate::serve::DaemonProfile::Native)
        .or_else(|| live.first())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no daemon profile is available for onboarding"))?;
    let original_instance = permission_instance(&presenter)
        .ok_or_else(|| {
            anyhow::anyhow!("{} returned no process instance", presenter.endpoint.label)
        })?
        .to_owned();
    let panel_completion_baseline = presenter
        .payload
        .get("panel")
        .and_then(|value| value.get("completion"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    let present_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match send_permission_control(&presenter.endpoint, "permissions_present") {
            Ok(_) => break,
            Err(error) if std::time::Instant::now() < present_deadline => {
                let panel_is_visible = query_permission_daemon(&presenter.endpoint)?
                    .and_then(|state| state.payload.get("panel")?.get("visible")?.as_bool())
                    .unwrap_or(false);
                if panel_is_visible {
                    break;
                }
                let _ = error;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }
    println!("Complete Accessibility and Screen Recording in the BrigadierCu panel.");
    let presenter_fresh = wait_for_fresh_permission_state(
        &presenter.endpoint,
        &original_instance,
        true,
        panel_completion_baseline,
    )?;

    let mut refresh_endpoints: Vec<PermissionDaemonEndpoint> =
        live.iter().map(|state| state.endpoint.clone()).collect();
    for state in discover_permission_daemons()? {
        if !refresh_endpoints
            .iter()
            .any(|endpoint| endpoint.socket == state.endpoint.socket)
        {
            refresh_endpoints.push(state.endpoint);
        }
    }
    for endpoint in refresh_endpoints {
        if endpoint.socket == presenter_fresh.endpoint.socket {
            continue;
        }
        let state = wait_for_permission_daemon(&endpoint, std::time::Duration::from_secs(10))?;
        refresh_permission_daemon(&state)?;
    }

    println!("BrigadierCu has Accessibility and Screen Recording for every live profile.");
    Ok(())
}

/// `brigadier-cu diagnose` — print a paste-able bundle-path / install-layout / TCC report.
///
/// Mirrors Swift `DiagnoseCommand`. Covers:
///   - running process identity (path, pid, version)
///   - codesign info (cdhash, team-id, authority) via `codesign -dvvv`
///   - AX + screen recording TCC status (check_permissions tool)
///   - install layout (/Applications/Brigadier Computer Use.app, ~/.local/bin/brigadier-cu)
///   - TCC DB rows for ai.brigadier.computer-use (sqlite3, best-effort)
///   - config + state paths with existence booleans
pub fn run_diagnose_cmd(registry: std::sync::Arc<ToolRegistry>) {
    let sections = [
        diagnose_runtime_section(),
        diagnose_signature_section(),
        diagnose_tcc_section(registry),
        diagnose_install_layout_section(),
        diagnose_tcc_db_section(),
        diagnose_config_paths_section(),
    ];
    println!("{}", sections.join("\n\n"));
}

fn diagnose_runtime_section() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_else(|| "<unknown>".into());
    let argv0 = std::env::args()
        .next()
        .unwrap_or_else(|| "<unknown>".into());
    let pid = std::process::id();
    let version = env!("CARGO_PKG_VERSION");
    format!(
        "## running process\n\
         version:        {version}\n\
         argv[0]:        {argv0}\n\
         executablePath: {exe}\n\
         pid:            {pid}"
    )
}

fn diagnose_signature_section() -> String {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(str::to_owned))
        .unwrap_or_default();

    let out = std::process::Command::new("codesign")
        .args(["-dvvv", &exe])
        .output();

    let mut cdhash = "<unknown>".to_owned();
    let mut team_id = "<none — ad-hoc signed?>".to_owned();
    let mut authority = "<none — ad-hoc signed?>".to_owned();

    if let Ok(out) = out {
        // codesign prints to stderr
        let text = String::from_utf8_lossy(&out.stderr);
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("CDHash=") {
                cdhash = rest.trim().to_owned();
            } else if let Some(rest) = line.strip_prefix("TeamIdentifier=") {
                team_id = rest.trim().to_owned();
            } else if let Some(rest) = line.strip_prefix("Authority=") {
                // Only take the first (leaf certificate) authority line.
                if authority == "<none — ad-hoc signed?>" {
                    authority = rest.trim().to_owned();
                }
            }
        }
    }

    format!(
        "## running process signature\n\
         cdhash:    {cdhash}\n\
         teamID:    {team_id}\n\
         authority: {authority}"
    )
}

fn diagnose_tcc_section(registry: std::sync::Arc<ToolRegistry>) -> String {
    // Call check_permissions in-process (quick, read-only).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();

    let (ax, sr) = if let Ok(rt) = rt {
        rt.block_on(async {
            let result = registry
                .invoke(
                    "check_permissions",
                    serde_json::Value::Object(Default::default()),
                )
                .await;
            if let Some(sc) = &result.structured_content {
                let ax = sc
                    .get("accessibility")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let sr = sc
                    .get("screen_recording")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                (ax, sr)
            } else {
                (false, false)
            }
        })
    } else {
        (false, false)
    };

    format!(
        "## tcc probes (live process)\n\
         accessibility     (AXIsProcessTrusted): {ax}\n\
         screen recording  (SCShareableContent):  {sr}\n\n\
         if the UI disagrees with these booleans the live process is fine —\n\
         the issue is elsewhere (wrong bundle granted, stale cdhash, etc)."
    )
}

fn diagnose_install_layout_section() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mut lines = vec!["## install layout".to_owned()];

    let app_path = "/Applications/Brigadier Computer Use.app";
    let app_exists = std::path::Path::new(app_path).exists();
    lines.push(format!("bundle:  {app_path}   exists={app_exists}"));
    if app_exists {
        // Codesign info for the bundle.
        if let Ok(out) = std::process::Command::new("codesign")
            .args(["-dvvv", app_path])
            .output()
        {
            let text = String::from_utf8_lossy(&out.stderr);
            let mut cdhash = "<unknown>";
            let mut team_id = "<none>";
            let mut authority = "<none>";
            for line in text.lines() {
                if let Some(r) = line.strip_prefix("CDHash=") {
                    cdhash = r.trim();
                } else if let Some(r) = line.strip_prefix("TeamIdentifier=") {
                    team_id = r.trim();
                } else if line.starts_with("Authority=") && authority == "<none>" {
                    authority = line.trim_start_matches("Authority=").trim();
                }
            }
            lines.push(format!("  cdhash:    {cdhash}"));
            lines.push(format!("  teamID:    {team_id}"));
            lines.push(format!("  authority: {authority}"));
        }
    }

    let cli_paths = [
        ("symlink", format!("{home}/.local/bin/brigadier-cu")),
        ("legacy symlink", "/usr/local/bin/brigadier-cu".to_owned()),
    ];
    for (label, path) in &cli_paths {
        let exists = std::path::Path::new(path).exists();
        lines.push(format!("{label}: {path}   exists={exists}"));
        if exists {
            if let Ok(target) = std::fs::read_link(path) {
                lines.push(format!("  resolves to: {}", target.display()));
            }
        }
    }

    let stale = format!("{home}/Applications/Brigadier Computer Use.app");
    if std::path::Path::new(&stale).exists() {
        lines.push(format!(
            "stale:   {stale}   \u{2190} old install-local.sh path, consider removing"
        ));
    }

    lines.join("\n")
}

fn diagnose_tcc_db_section() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let db = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
    let sql = "SELECT service, client, client_type, auth_value, auth_reason, \
               hex(csreq) AS csreq_hex FROM access WHERE client='ai.brigadier.computer-use';";

    let mut lines = vec!["## tcc database rows for ai.brigadier.computer-use".to_owned()];
    lines.push(format!(
        "(reading {db} — best-effort; system TCC DB requires FDA)"
    ));
    lines.push(String::new());

    match std::process::Command::new("sqlite3")
        .args(["-header", "-column", &db, sql])
        .output()
    {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let trimmed = stdout.trim();
            if !trimmed.is_empty() {
                lines.push(trimmed.to_owned());
            } else if out.status.code() != Some(0) && !stderr.trim().is_empty() {
                lines.push(format!("(sqlite3 failed: {})", stderr.trim()));
            } else {
                lines.push("(no rows — either grants never made it to TCC, or they live".into());
                lines.push(" in the system DB which requires Full Disk Access to read.)".into());
            }
        }
        Err(e) => {
            lines.push(format!("(could not launch sqlite3: {e})"));
        }
    }

    lines.push(String::new());
    lines.push("# auth_value legend: 0=denied  2=allowed".into());
    lines.push("# services: kTCCServiceAccessibility, kTCCServiceScreenCapture".into());
    lines.join("\n")
}

fn diagnose_config_paths_section() -> String {
    let home = crate::home::home();
    let paths: &[(&str, String)] = &[
        ("helper home", home.to_string_lossy().into_owned()),
        (
            "config",
            home.join("config.json").to_string_lossy().into_owned(),
        ),
        (
            "socket",
            home.join("cu.sock").to_string_lossy().into_owned(),
        ),
    ];
    let mut lines = vec!["## config + state paths".to_owned()];
    for (label, path) in paths {
        let exists = std::path::Path::new(path).exists();
        lines.push(format!("{:<18} exists={exists}   {path}", label));
    }
    lines.join("\n")
}

/// Path to the persistent JSON config file (`~/.brigadier-cu/config.json`).
fn config_file_path() -> std::path::PathBuf {
    crate::home::home().join("config.json")
}

/// Read persisted config from disk.  Returns an empty object if absent/unreadable.
fn read_config_file() -> serde_json::Value {
    let path = config_file_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

/// Write a single key/value into the persisted config file.
fn write_config_file(key: &str, value: &serde_json::Value) {
    let path = config_file_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut cfg = read_config_file();
    if let serde_json::Value::Object(ref mut map) = cfg {
        map.insert(key.to_owned(), value.clone());
    }
    if let Ok(json) = serde_json::to_string_pretty(&cfg) {
        let _ = std::fs::write(&path, json);
    }
}

/// `brigadier-cu config [show|get|set|reset] [key] [value]`
///
/// Thin wrapper around the `get_config` / `set_config` MCP tools.
/// Forwards to the daemon when one is reachable, otherwise runs in-process.
pub fn run_config_cmd(
    registry: std::sync::Arc<ToolRegistry>,
    subcommand: Option<&str>,
    key: Option<&str>,
    value: Option<&str>,
    socket: Option<&str>,
) {
    let socket_path = socket
        .map(str::to_owned)
        .unwrap_or_else(crate::serve::default_socket_path);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    match subcommand.unwrap_or("show") {
        "show" | "" => {
            // Print full config as pretty JSON.
            let config =
                rt.block_on(async { registry.invoke("get_config", serde_json::json!({})).await });
            if let Some(sc) = config.structured_content {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string())
                );
            } else {
                eprintln!("get_config: no structured content returned");
                process::exit(1);
            }
        }

        "get" => {
            let key = match key {
                Some(k) => k,
                None => {
                    eprintln!("Usage: brigadier-cu config get <key>");
                    eprintln!("Keys: allow_unrestricted_app_state, capture_mode, max_image_dimension, version, platform");
                    process::exit(64);
                }
            };
            // Try daemon first.
            if crate::serve::is_daemon_listening(&socket_path) {
                let req = crate::serve::DaemonRequest {
                    method: "call".into(),
                    name: Some("get_config".into()),
                    args: Some(serde_json::json!({})),
                    // CLI `config get` reads the persisted global (anonymous).
                    session_id: None,
                };
                if let Ok(resp) = crate::serve::send_request(&socket_path, &req) {
                    if resp.ok {
                        if let Some(result) = resp.result {
                            if let Some(sc) = result.get("structuredContent") {
                                if let Some(v) = sc.get(key) {
                                    println!("{v}");
                                    return;
                                }
                            }
                        }
                    }
                }
            }
            // In-process: merge persisted file config over in-memory defaults.
            let config =
                rt.block_on(async { registry.invoke("get_config", serde_json::json!({})).await });
            let mut sc = match config.structured_content {
                Some(v) => v,
                None => {
                    eprintln!("get_config: no structured content returned");
                    process::exit(1);
                }
            };
            // Overlay persisted values from the config file.
            let file_cfg = read_config_file();
            if let (serde_json::Value::Object(sc_map), serde_json::Value::Object(file_map)) =
                (&mut sc, file_cfg)
            {
                for (k, v) in file_map {
                    sc_map.insert(k, v);
                }
            }
            // Support dotted key paths like "agent_cursor.enabled".
            let v = if key.contains('.') {
                let (parent, child) = key.split_once('.').unwrap();

                sc.get(parent).and_then(|obj| obj.get(child)).cloned()
            } else {
                sc.get(key).cloned()
            };
            if let Some(v) = v {
                println!(
                    "{}",
                    match &v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    }
                );
            } else {
                eprintln!("Unknown config key: {key}");
                eprintln!("Available keys: allow_unrestricted_app_state, capture_mode, max_image_dimension, version, platform, agent_cursor.enabled");
                process::exit(64);
            }
        }

        "set" => {
            let key = match key {
                Some(k) => k,
                None => {
                    eprintln!("Usage: brigadier-cu config set <key> <value>");
                    process::exit(64);
                }
            };
            let value = match value {
                Some(v) => v,
                None => {
                    eprintln!("Usage: brigadier-cu config set {key} <value>");
                    process::exit(64);
                }
            };
            // Parse value: try JSON, fall back to string.
            let parsed_value: serde_json::Value = serde_json::from_str(value)
                .unwrap_or_else(|_| serde_json::Value::String(value.to_owned()));
            let args = serde_json::json!({ key: parsed_value });

            // Try daemon first.
            if crate::serve::is_daemon_listening(&socket_path) {
                let req = crate::serve::DaemonRequest {
                    method: "call".into(),
                    name: Some("set_config".into()),
                    args: Some(args.clone()),
                    // CLI `config set` is anonymous → writes the persisted
                    // global default (the only writer of the on-disk config).
                    session_id: None,
                };
                if let Ok(resp) = crate::serve::send_request(&socket_path, &req) {
                    if resp.ok {
                        println!("Config updated.");
                        // Show current state.
                        let req2 = crate::serve::DaemonRequest {
                            method: "call".into(),
                            name: Some("get_config".into()),
                            args: Some(serde_json::json!({})),
                            session_id: None,
                        };
                        if let Ok(r2) = crate::serve::send_request(&socket_path, &req2) {
                            if let Some(result) = r2.result {
                                if let Some(sc) = result.get("structuredContent") {
                                    println!(
                                        "{}",
                                        serde_json::to_string_pretty(sc).unwrap_or_default()
                                    );
                                }
                            }
                        }
                        return;
                    } else if let Some(e) = resp.error {
                        eprintln!("{e}");
                        process::exit(1);
                    }
                }
            }
            // In-process.
            let result = rt.block_on(async { registry.invoke("set_config", args).await });
            if result.is_error.unwrap_or(false) {
                for item in &result.content {
                    if let cu_core::protocol::Content::Text { text, .. } = item {
                        eprintln!("{text}");
                    }
                }
                process::exit(1);
            }
            // Persist the value to disk so future CLI invocations can read it.
            write_config_file(key, &parsed_value);
            // Print updated config.
            let config =
                rt.block_on(async { registry.invoke("get_config", serde_json::json!({})).await });
            if let Some(sc) = config.structured_content {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string())
                );
            }
        }

        "reset" => {
            // Reset to defaults by calling set_config with empty args.
            // The set_config tool keeps existing values if keys are absent,
            // so we send the known defaults explicitly.
            let defaults = serde_json::json!({
                "capture_mode": "ax",
                "max_image_dimension": 0,
                "allow_unrestricted_app_state": true
            });
            let result = rt.block_on(async { registry.invoke("set_config", defaults).await });
            if result.is_error.unwrap_or(false) {
                eprintln!("config reset failed");
                process::exit(1);
            }
            println!("Config reset to defaults.");
            let config =
                rt.block_on(async { registry.invoke("get_config", serde_json::json!({})).await });
            if let Some(sc) = config.structured_content {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&sc).unwrap_or_else(|_| sc.to_string())
                );
            }
        }

        other => {
            eprintln!("Unknown config subcommand '{other}'. Valid: show, get <key>, set <key> <value>, reset");
            process::exit(64);
        }
    }
}

/// `brigadier-cu doctor` — run platform-aware diagnostic probes and emit a
/// structured report.
///
/// See [`crate::doctor`] for the probe catalog. Output is plain text by
/// default; `--json` switches to a machine-readable shape for scripting.
/// Exit code is `0` when every probe is `[ok]` or `[warn]`, non-zero when
/// at least one `[err]` probe failed (e.g. the binary cannot resolve its
/// own install dir).
pub fn run_doctor_cmd(json: bool) {
    let report = crate::doctor::run();

    if json {
        let val = report.to_json();
        let out = serde_json::to_string_pretty(&val).unwrap_or_else(|_| val.to_string());
        println!("{out}");
    } else {
        print!("{}", report.to_text());
    }

    if report.has_errors() {
        process::exit(1);
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// Read JSON from stdin when stdin is a pipe (non-interactive). Returns `None`
/// when stdin is a terminal or the input isn't valid JSON.
/// Matches Swift's "If omitted, reads from stdin when stdin is a pipe."
///
/// Strips a leading UTF-8 BOM (`U+FEFF`, bytes `EF BB BF`) before parsing.
/// Without this, payloads written via PowerShell 5.1's
/// `Set-Content -Encoding utf8` (which silently prepends a BOM) parse as
/// invalid JSON and the call falls through to default-args, producing
/// confusing "Missing required integer field" errors despite the caller
/// having sent a valid-looking payload. See the 2026-05-23 dogfood journal.
fn read_stdin_json() -> Option<serde_json::Value> {
    use std::io::{self, IsTerminal, Read};
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut buf = String::new();
    stdin.lock().read_to_string(&mut buf).ok()?;
    let trimmed = buf.trim();
    // U+FEFF is one character (3 bytes UTF-8) — `str::strip_prefix` matches by
    // chars, so a single `'\u{feff}'` is the right comparand.
    let stripped = trimmed.strip_prefix('\u{feff}').unwrap_or(trimmed);
    serde_json::from_str(stripped).ok()
}

#[cfg(test)]
mod stdin_bom_tests {
    /// Manual cross-check that the BOM-stripping logic round-trips correctly
    /// without needing a real stdin pipe.
    #[test]
    fn strip_prefix_handles_utf8_bom() {
        let with_bom = "\u{feff}{\"pid\":42}";
        let stripped = with_bom.strip_prefix('\u{feff}').unwrap_or(with_bom);
        assert_eq!(stripped, "{\"pid\":42}");
        let v: serde_json::Value = serde_json::from_str(stripped).unwrap();
        assert_eq!(v["pid"], 42);
    }

    #[test]
    fn strip_prefix_no_op_when_no_bom() {
        let plain = "{\"pid\":7}";
        let stripped = plain.strip_prefix('\u{feff}').unwrap_or(plain);
        assert_eq!(stripped, plain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    fn permission_state_with_panel(
        completion: u64,
        visible: bool,
        outcome: Option<&str>,
        recovery: &str,
    ) -> PermissionDaemonState {
        PermissionDaemonState {
            endpoint: permission_daemon_endpoints().remove(0),
            payload: serde_json::json!({
                "panel": {
                    "completion": completion,
                    "visible": visible,
                    "last_outcome": outcome,
                },
                "recovery": { "state": recovery },
            }),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_recovery_ignores_stale_dismissal_until_panel_completes_again() {
        let stale = permission_state_with_panel(4, false, Some("dismissed"), "queued");
        assert!(!permission_onboarding_dismissed(&stale, false, 4));

        let reopened = permission_state_with_panel(4, true, None, "dismissed");
        assert!(!permission_onboarding_dismissed(&reopened, false, 4));

        let completed = permission_state_with_panel(5, false, Some("dismissed"), "dismissed");
        assert!(permission_onboarding_dismissed(&completed, false, 4));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_recovery_detects_dismissal_after_process_refresh() {
        let fresh_process = permission_state_with_panel(1, false, Some("dismissed"), "idle");
        assert!(permission_onboarding_dismissed(&fresh_process, true, 9));

        let still_visible = permission_state_with_panel(0, true, None, "idle");
        assert!(!permission_onboarding_dismissed(&still_visible, true, 9));
    }

    #[test]
    fn silent_permission_status_does_not_treat_unknown_probe_as_failure() {
        let lines = permission_status_lines(&serde_json::json!({
            "accessibility": true,
            "screen_recording": true,
            "screen_recording_capturable": null,
            "screen_recording_probe_performed": false,
            "source": { "attribution": "driver-daemon" }
        }));
        let output = lines.join("\n");
        assert!(output.contains("not performed"));
        assert!(!output.contains("live capture probe failed"));
    }

    #[test]
    fn live_permission_probe_false_warns_but_true_does_not() {
        for (capturable, should_warn) in [(false, true), (true, false)] {
            let lines = permission_status_lines(&serde_json::json!({
                "accessibility": true,
                "screen_recording": true,
                "screen_recording_capturable": capturable,
                "screen_recording_probe_performed": true,
                "source": { "attribution": "driver-daemon" }
            }));
            assert_eq!(
                lines.join("\n").contains("live capture probe failed"),
                should_warn,
            );
        }
    }

    #[test]
    fn probe_timeout_is_shown_as_capture_timeout_not_as_failure_or_not_performed() {
        let lines = permission_status_lines(&serde_json::json!({
            "accessibility": true,
            "screen_recording": true,
            "screen_recording_capturable": null,
            "screen_recording_probe_performed": true,
            "screen_recording_capture_error": {
                "code": "capture_timeout",
                "operation": "screenshot_capture",
                "elapsed_ms": 10000,
                "budget_ms": 10000,
                "hint": "A system dialog may be waiting."
            },
            "source": { "attribution": "driver-daemon" }
        }));
        let output = lines.join("\n");
        assert!(output.contains("capture_timeout"), "{output}");
        assert!(output.contains("screenshot_capture"), "{output}");
        assert!(output.contains("10000 ms"), "{output}");
        assert!(!output.contains("not performed"), "{output}");
        assert!(!output.contains("live capture probe failed"), "{output}");
    }

    // ── Surface 7b: serve --owner-pid ───────────────────────────────────────

    #[test]
    fn owner_pid_accepts_only_a_positive_process_id() {
        assert_eq!(parse_owner_pid("4242"), Ok(4242));
        for bad in ["", "0", "-1", "abc", "12x", "99999999999"] {
            assert!(parse_owner_pid(bad).is_err(), "{bad:?} must be refused");
        }
    }

    // ── Surface 8: manifest shape ───────────────────────────────────────────
}

fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let flat: String = trimmed
        .split("\n\n")
        .next()
        .unwrap_or(trimmed)
        .split('\n')
        .collect::<Vec<_>>()
        .join(" ");
    let mut sentence = String::new();
    let mut prev = ' ';
    for ch in flat.chars() {
        if (prev == '.' || prev == '?' || prev == '!') && ch == ' ' {
            break;
        }
        sentence.push(ch);
        prev = ch;
    }
    let mut s = sentence.trim().to_string();
    if s.ends_with('.') {
        s.pop();
    }
    s
}
