//! Stdio MCP proxy that forwards `tools/list` and `tools/call` through
//! the running `brigadier-cu serve` daemon over its Unix socket.
//!
//! This is the runtime half of the TCC auto-relaunch path (issue #1525,
//! mirror of Swift PR #1479). When `brigadier-cu mcp` is invoked from
//! an IDE terminal — Claude Code, Cursor, VS Code, Warp — macOS TCC
//! attributes the process to the calling terminal, not to
//! `Brigadier Computer Use.app`. The MCP client side sees a normal stdio server,
//! but every AX probe silently fails because the binary is running
//! against the wrong bundle id.
//!
//! The fix: detect that context (see `crate::bundle`), ensure a daemon
//! is running under `LaunchServices` (which gives it the right TCC
//! attribution), then proxy every MCP request through the daemon's
//! socket. The MCP client never sees the redirection — same JSON-RPC
//! envelope, same tool semantics.
//!
//! Why this lives in `brigadier-cu` and not `mcp-server`:
//!   `crate::mcp::run` already speaks JSON-RPC over stdio
//!   against an in-process `ToolRegistry`. The proxy speaks the same
//!   protocol on the client side but the server side is the daemon's
//!   line-delimited JSON UDS protocol, owned by `crate::serve`.
//!   Putting the proxy here avoids `mcp-server → brigadier-cu` reverse
//!   coupling.

use std::sync::Arc;

use crate::mcp::initialize_result;
use cu_core::protocol::{Request, Response};
#[cfg(target_os = "macos")]
use cu_core::protocol::{ToolCall, ToolResult};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

use crate::serve::{
    never_delivered, send_request, serialize_request, DaemonProfile, DaemonRequest, DaemonResponse,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum ControlConnectionState {
    Connecting,
    Ready,
    Rejected(String),
}

const PERFORM_ACTIONS_TOOL: &str = "perform_actions";
const MAX_ACTION_GROUP_SIZE: usize = 20;
const ACTION_GROUP_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "type_text",
    "press_key",
    "hotkey",
    "scroll",
    "drag",
    "set_value",
    "move_cursor",
];

fn perform_actions_tool_entry() -> serde_json::Value {
    serde_json::json!({
        "name": PERFORM_ACTIONS_TOOL,
        "description": "Execute up to 20 already-grounded UI actions in order inside one \
            persistent Computer Use proxy call. Use this after one get_window_state when \
            the referenced controls remain stable (for example, a Calculator button \
            sequence), then verify the completed group with one fresh snapshot. Each step \
            reuses the existing element-token cache and the same visible agent cursor, \
            avoiding a new model/MCP round trip and accessibility-tree scan per click. \
            Do not group navigation, modal-opening, or layout-changing actions whose later \
            controls require a fresh snapshot.",
        "inputSchema": {
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "actions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": MAX_ACTION_GROUP_SIZE,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "tool": {
                                "type": "string",
                                "enum": ACTION_GROUP_TOOLS,
                            },
                            "arguments": {
                                "type": "object",
                            },
                        },
                        "required": ["tool", "arguments"],
                    },
                },
                "stop_on_error": {
                    "type": "boolean",
                    "description": "Stop before later actions after the first failed step. Default true.",
                },
            },
            "required": ["actions"],
        },
        "annotations": {
            "readOnlyHint": false,
            "destructiveHint": true,
            "idempotentHint": false,
            "openWorldHint": false,
        },
        "capabilities": ["input.action_group"],
    })
}

fn with_proxy_tools(mut tools_list: serde_json::Value) -> serde_json::Value {
    if let Some(tools) = tools_list
        .get_mut("tools")
        .and_then(serde_json::Value::as_array_mut)
    {
        if !tools.iter().any(|tool| {
            tool.get("name").and_then(serde_json::Value::as_str) == Some(PERFORM_ACTIONS_TOOL)
        }) {
            tools.push(perform_actions_tool_entry());
        }
    }
    tools_list
}

/// Run the MCP stdio proxy. Reads JSON-RPC lines from stdin, forwards
/// the body of each `tools/list` / `tools/call` to the daemon at
/// `socket_path`, and writes the daemon's response back as a proper
/// JSON-RPC envelope.
///
/// Mirrors `crate::mcp::run`'s control flow exactly — same
/// EOF + parse-error + notification handling — only the per-method
/// branches change.
///
/// Fails fast if the daemon isn't reachable, so MCP clients see a
/// clear startup error instead of a "successful" handshake that
/// advertises zero tools and then errors on every call. Matches
/// Swift `makeProxy`'s `fetchProxyToolList` pre-check.
pub async fn run_proxy(
    socket_path: String,
    claude_code_compat: bool,
    expected_profile: DaemonProfile,
) -> anyhow::Result<()> {
    // Mint this MCP session's identity once at proxy startup. One proxy process
    // == one MCP session; the daemon outlives it. We stamp this id on every
    // forwarded request so the daemon can OWN and CLEAN UP this session's
    // state (recording, config overrides) and tear it down on disconnect via
    // a `session_end` signal. Dep-free `pid + start-nanos` is sufficient for
    // daemon-local uniqueness over this proxy's lifetime (no `uuid` crate dep
    // for one mint).
    let minted_session_id = mint_session_id();
    let configured_default_session = configured_default_session();
    let (session_id, managed_session) =
        proxy_session_identity(&minted_session_id, configured_default_session.as_deref());
    debug!(session_id = %session_id, "proxy session minted");
    if managed_session {
        debug!(
            session_id = %session_id,
            "proxy session bound to host-managed default identity"
        );
    }

    // Serve `initialize` and `tools/list` WITHOUT the daemon so the
    // permission-requesting daemon stays DORMANT until the agent actually
    // invokes a tool.
    //
    // The "control" connection referenced below is the reaper: it holds one
    // long-lived socket to the daemon and sends a single `session_begin`; when
    // the proxy exits (graceful stdin EOF) OR is SIGKILLed/crashes, the kernel
    // closes it and the daemon fires `session_end` for this `session_id`,
    // tearing down every piece of state this session owns (overlay cursor,
    // config overrides, recording).
    //
    let (control_ready_tx, control_ready_rx) =
        tokio::sync::watch::channel(ControlConnectionState::Connecting);
    // macOS tracks daemon/reaper ownership separately from the permission
    // onboarding milestone. A read-only permission probe may start the daemon
    // promptly without allowing a later driving action to skip the grant wait.
    #[cfg(target_os = "macos")]
    let mut daemon_state = DaemonStartState::default();

    // macOS: build the tool list from the in-process registry — a pure,
    // permission-free operation — and launch the daemon (plus the reaper) lazily
    // on the FIRST `tools/call` (see `ensure_daemon_started`). Nothing prompts
    // merely because an agent registered this MCP server at session start.
    //
    // Other platforms: there is no lazy `open -a` launch path, so the caller
    // guarantees a daemon is already up. Fetch the list from it and start the
    // reaper immediately (unchanged behaviour).
    #[cfg(target_os = "macos")]
    let bootstrap_tools_list = {
        let registry = crate::build_macos_registry_with_compat(claude_code_compat);
        Arc::new(with_proxy_tools(registry.tools_list()))
    };

    #[cfg(target_os = "macos")]
    let (daemon_lifecycle_tx, mut daemon_lifecycle_rx) =
        tokio::sync::mpsc::unbounded_channel::<DaemonLifecycleEvent>();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut lines = BufReader::new(stdin).lines();
    let mut writer = tokio::io::BufWriter::new(stdout);

    loop {
        // The persistent control connection is also the authoritative daemon
        // lifetime signal. A helper can exit and finish relaunching between
        // two MCP calls, leaving the same socket path reachable; a transient
        // `is_daemon_listening` probe cannot identify that replacement. Its
        // EOF event invalidates the generation/cache before the next request.
        // `next_line` is cancellation-safe, so selecting it against lifecycle
        // events cannot discard a partially received JSON-RPC line.
        #[cfg(target_os = "macos")]
        let line = tokio::select! {
            biased;
            event = daemon_lifecycle_rx.recv() => {
                if let Some(event) = event {
                    daemon_state.observe_control_connection_end(event.generation);
                }
                continue;
            }
            line = lines.next_line() => line?,
        };
        let Some(line) = line else {
            break; // EOF — MCP client disconnected (stdin closed).
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        debug!(raw = trimmed, "→ proxy request");

        let mut journal_call: Option<(String, Option<serde_json::Value>, std::time::Instant)> =
            None;
        let response = match serde_json::from_str::<Request>(trimmed) {
            Err(e) => {
                error!("JSON parse error: {e}");
                Response::parse_error()
            }
            Ok(req) if req.is_notification() => {
                // Notifications get dropped, same as `server::run`.
                continue;
            }
            Ok(req) => {
                let id = req.id.clone().unwrap_or(serde_json::Value::Null);
                journal_call = (req.method == "tools/call" && crate::journal::enabled())
                    .then(|| req.tool_call().ok())
                    .flatten()
                    .map(|call| (call.name, Some(call.args), std::time::Instant::now()));

                // macOS: bring the daemon (and the reaper control connection) up
                // only after a tools/call passes permission-free protocol,
                // roster, and schema admission. initialize/tools/list remain
                // local so registering the server cannot trigger TCC prompts.
                #[cfg(target_os = "macos")]
                let platform_response = if req.method == "tools/call" {
                    let tools_list = daemon_state.tools_list_or(&bootstrap_tools_list);
                    match admit_bootstrap_tool_call(&req, tools_list) {
                        BootstrapToolCallAdmission::InvalidParams(error) => {
                            Response::error(id, -32602, format!("Invalid params: {error}"))
                        }
                        BootstrapToolCallAdmission::Rejected(result) => {
                            response_from_tool_result(id, result)
                        }
                        BootstrapToolCallAdmission::Ready {
                            call,
                            wait_for_grants,
                        } => {
                            let read_only = tool_is_read_only(tools_list, &call.name);
                            if crate::lifecycle::is_off(&crate::lifecycle::home()) {
                                response_from_tool_result(
                                    id,
                                    ToolResult::error(crate::lifecycle::OFF_MESSAGE),
                                )
                            } else {
                                match ensure_daemon_started(
                                    &socket_path,
                                    &mut daemon_state,
                                    &session_id,
                                    claude_code_compat,
                                    expected_profile,
                                    &control_ready_tx,
                                    wait_for_grants,
                                    &daemon_lifecycle_tx,
                                )
                                .await
                                {
                                    Err(error) => response_from_tool_result(
                                        id,
                                        ToolResult::error(unavailable_message(&error)),
                                    ),
                                    Ok(()) => {
                                        let first = forward_tool_call(
                                            id.clone(),
                                            call.name.clone(),
                                            call.args.clone(),
                                            &socket_path,
                                            &session_id,
                                            &control_ready_rx,
                                            read_only,
                                        )
                                        .await;
                                        match first {
                                            Forwarded::Done(response) => response,
                                            // The daemon went away under a call that is safe to
                                            // send again. Bring up a replacement and retry it
                                            // once; report the original failure only if recovery
                                            // fails.
                                            Forwarded::Lost(failure) => {
                                                crate::journal::record_event(
                                                    "helper_recovery",
                                                    "daemon lost during a tool call; retrying once",
                                                );
                                                daemon_state.observe_daemon_loss();
                                                match ensure_daemon_started(
                                                    &socket_path,
                                                    &mut daemon_state,
                                                    &session_id,
                                                    claude_code_compat,
                                                    expected_profile,
                                                    &control_ready_tx,
                                                    wait_for_grants,
                                                    &daemon_lifecycle_tx,
                                                )
                                                .await
                                                {
                                                    Err(error) => {
                                                        drop(failure);
                                                        response_from_tool_result(
                                                            id,
                                                            ToolResult::error(unavailable_message(
                                                                &error,
                                                            )),
                                                        )
                                                    }
                                                    Ok(()) => match forward_tool_call(
                                                        id,
                                                        call.name,
                                                        call.args,
                                                        &socket_path,
                                                        &session_id,
                                                        &control_ready_rx,
                                                        read_only,
                                                    )
                                                    .await
                                                    {
                                                        Forwarded::Done(response)
                                                        | Forwarded::Lost(response) => response,
                                                    },
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    let tools_list = daemon_state.tools_list_or(&bootstrap_tools_list);
                    handle_proxy_request(
                        req,
                        id,
                        &socket_path,
                        tools_list,
                        &session_id,
                        &control_ready_rx,
                    )
                    .await
                };

                platform_response
            }
        };

        if let Some((tool, args, started)) = journal_call {
            if let Ok(value) = serde_json::to_value(&response) {
                crate::journal::record_tool_call(
                    &tool,
                    args.as_ref(),
                    &session_id,
                    &format!("{expected_profile:?}"),
                    started.elapsed(),
                    &value,
                );
            }
        }

        let serialized = serde_json::to_string(&response).unwrap_or_else(|e| {
            format!(
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#
            )
        });
        debug!(raw = %serialized, "← proxy response");

        writer.write_all(serialized.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    // Reached on a clean stdin EOF (the `n == 0` break above) — the normal
    // "MCP client disconnected" seam. Session teardown is NO LONGER done here:
    // it's fully subsumed by the persistent control connection spawned at
    // startup. On any proxy exit — graceful stdin EOF (this path), an I/O
    // error propagated via `?`, OR a SIGKILL/crash — the kernel closes the
    // control socket, the daemon's reader hits EOF, and it fires
    // `session_end(session_id)` once (idempotent). That single path reliably
    // covers the ungraceful-death case the old best-effort exit hook missed.
    Ok(())
}

/// Own the proxy's single long-lived control connection. Connects directly to
/// the daemon socket (its OWN async open — `send_request` is sync, blocking,
/// and one-shot, so it cannot be reused here), sends one `session_begin` line
/// carrying `session_id`, then parks in a read loop until the connection
/// closes. It never writes again. The daemon records `session_id` from
/// `session_begin` and fires `session_end` when this connection EOFs — which
/// the kernel triggers on proxy exit AND on kill -9.
///
/// When the daemon closes the connection during a permission-triggered re-exec,
/// reconnect with the same session identity and send `session_begin` again.
/// The old daemon reaps the old connection before it exits; the replacement
/// daemon then owns cleanup for the reconnected session. The task ends only
/// when the proxy runtime drops it, which also closes the current connection
/// and triggers the replacement daemon's EOF cleanup.
async fn run_control_connection(
    socket_path: String,
    session_id: String,
    expected_profile: DaemonProfile,
    readiness: tokio::sync::watch::Sender<ControlConnectionState>,
    mut lifecycle: Option<(
        tokio::sync::mpsc::UnboundedSender<DaemonLifecycleEvent>,
        u64,
    )>,
) {
    let begin = DaemonRequest {
        method: "session_begin".into(),
        name: None,
        args: None,
        session_id: Some(session_id.clone()),
    };
    let line = match serialize_request(&begin) {
        Ok(s) => s + "\n",
        Err(e) => {
            warn!("control connection: serialize session_begin failed: {e}");
            return;
        }
    };

    #[cfg(unix)]
    {
        loop {
            let connected = match run_unix_control_connection_once(
                &socket_path,
                &session_id,
                &line,
                expected_profile,
                &readiness,
            )
            .await
            {
                Ok(connected) => connected,
                Err(error) => {
                    let _ = readiness.send(ControlConnectionState::Rejected(error));
                    return;
                }
            };
            if connected {
                if let Some((sender, generation)) = lifecycle.as_mut() {
                    let _ = sender.send(DaemonLifecycleEvent {
                        generation: *generation,
                    });
                    *generation = generation.wrapping_add(1).max(1);
                }
            }
            debug!(
                session_id = %session_id,
                connected,
                "control connection unavailable; waiting to reconnect"
            );
            tokio::time::sleep(std::time::Duration::from_millis(if connected {
                50
            } else {
                250
            }))
            .await;
        }
    }
}

async fn wait_for_control_connection(
    mut readiness: tokio::sync::watch::Receiver<ControlConnectionState>,
) -> anyhow::Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        loop {
            match readiness.borrow().clone() {
                ControlConnectionState::Ready => return Ok(()),
                ControlConnectionState::Rejected(error) => anyhow::bail!(error),
                ControlConnectionState::Connecting => {}
            }
            readiness.changed().await.map_err(|_| {
                anyhow::anyhow!("daemon control-connection task stopped unexpectedly")
            })?;
        }
    })
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for the daemon session control connection"))?
}

fn validate_control_ack(line: &str, expected_profile: DaemonProfile) -> anyhow::Result<()> {
    let response: DaemonResponse = serde_json::from_str(line)
        .map_err(|error| anyhow::anyhow!("decode session_begin response: {error}"))?;
    if !response.ok {
        anyhow::bail!(
            "daemon rejected session_begin: {}",
            response
                .error
                .unwrap_or_else(|| "unknown daemon error".to_owned())
        );
    }
    let reported_profile = response
        .result
        .as_ref()
        .and_then(|result| result.get("profile"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("session_begin ACK did not report a daemon profile"))?;
    if reported_profile != expected_profile.as_str() {
        anyhow::bail!(
            "daemon profile mismatch: MCP requested `{expected_profile}`, but the daemon \
             reports `{reported_profile}`"
        );
    }
    Ok(())
}

#[cfg(unix)]
async fn run_unix_control_connection_once(
    socket_path: &str,
    session_id: &str,
    begin_line: &str,
    expected_profile: DaemonProfile,
    readiness: &tokio::sync::watch::Sender<ControlConnectionState>,
) -> Result<bool, String> {
    use tokio::net::UnixStream;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut stream = loop {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => break stream,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(error) => {
                debug!(
                    session_id,
                    "control connect failed while daemon restarts: {error}"
                );
                return Ok(false);
            }
        }
    };
    if let Err(error) = stream.write_all(begin_line.as_bytes()).await {
        debug!(
            session_id,
            "control connection: write session_begin failed: {error}"
        );
        return Ok(true);
    }
    let _ = stream.flush().await;
    let mut reader = BufReader::new(stream);
    let mut buffer = String::new();
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        reader.read_line(&mut buffer),
    )
    .await
    {
        Ok(Ok(bytes)) if bytes > 0 => {}
        Ok(Ok(_)) => {
            debug!(
                session_id,
                "control connection closed before session_begin ACK"
            );
            return Ok(true);
        }
        Ok(Err(error)) => {
            debug!(session_id, "control connection ACK read failed: {error}");
            return Ok(true);
        }
        Err(_) => {
            debug!(
                session_id,
                "control connection timed out waiting for session_begin ACK"
            );
            return Ok(true);
        }
    }
    validate_control_ack(buffer.trim(), expected_profile).map_err(|error| error.to_string())?;
    let _ = readiness.send(ControlConnectionState::Ready);
    debug!(
        session_id,
        "control connection established (session_begin acknowledged)"
    );

    loop {
        buffer.clear();
        match reader.read_line(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => continue,
        }
    }
    let _ = readiness.send(ControlConnectionState::Connecting);
    Ok(true)
}

/// Mint a session id unique among the live proxies sharing one daemon, for the
/// lifetime of this proxy process. `pid + process-start nanos` is dep-free and
/// sufficient: two proxies can't share a pid concurrently, and the nanos guard
/// disambiguates pid reuse across the daemon's lifetime. We deliberately avoid
/// the `uuid` crate — a single v4 mint isn't worth a new dependency.
fn mint_session_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("mcp-{pid}-{nanos}")
}

fn validate_daemon_profile_and_roster(
    result: &serde_json::Value,
    expected_profile: DaemonProfile,
    socket_path: &str,
) -> anyhow::Result<()> {
    let reported_profile = result
        .get("profile")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "daemon on {socket_path} did not report a tool profile. Stop that daemon and \
                 restart it with the current brigadier-cu binary before retrying."
            )
        })?;
    if reported_profile != expected_profile.as_str() {
        anyhow::bail!(
            "daemon profile mismatch on {socket_path}: MCP requested `{expected_profile}`, \
             but the daemon reports `{reported_profile}`. Stop it and restart the matching \
             `brigadier-cu serve` profile."
        );
    }

    Ok(())
}

/// One-shot daemon `list` over the UDS, reshaped into a MCP
/// `tools/list` result. The daemon now returns the full ToolDef
/// (`name`, `description`, `input_schema`, annotation hints) per
/// commit 3's `serve.rs` change.
///
/// macOS serves the in-process list only while its lazy proxy is dormant. Once
/// connected, it fetches this daemon-authored list once per observed daemon
/// generation and caches it for later `tools/list` requests.
fn fetch_tools_list_from_daemon(
    socket_path: &str,
    session_id: &str,
    expected_profile: DaemonProfile,
) -> anyhow::Result<serde_json::Value> {
    let req = DaemonRequest {
        method: "list".into(),
        name: None,
        args: None,
        session_id: Some(session_id.to_owned()),
    };
    let resp = send_request(socket_path, &req)?;
    if !resp.ok {
        anyhow::bail!(
            "daemon refused tool list on {socket_path}: {}",
            resp.error.unwrap_or_else(|| "(no error message)".into())
        );
    }
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("daemon list response missing `result` field"))?;
    validate_daemon_profile_and_roster(&result, expected_profile, socket_path)?;
    let tools_array = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("daemon list response missing `tools` array"))?;

    // Reshape the daemon's `{name, description, input_schema, read_only,
    // ..., capabilities}` envelope into MCP's `{name, description,
    // inputSchema, annotations: {...}, capabilities}` shape. Same
    // translation `ToolDef::to_list_entry` does for the in-process
    // path so MCP clients see identical tools/list output either way.
    //
    // `capabilities` is passed through verbatim when the daemon
    // provides it; older daemons that don't emit the field fall back
    // to a name-keyed lookup so the proxy still surfaces capability
    // metadata without an extra round-trip.
    let mcp_tools: Vec<serde_json::Value> = tools_array
        .iter()
        .map(|t| {
            let name = t.get("name").cloned().unwrap_or(serde_json::Value::Null);
            let description = t
                .get("description")
                .cloned()
                .unwrap_or(serde_json::Value::String(String::new()));
            let input_schema = t
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
            let read_only = t
                .get("read_only")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let destructive = t
                .get("destructive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let idempotent = t
                .get("idempotent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let open_world = t
                .get("open_world")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let capabilities = t
                .get("capabilities")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_else(|| {
                    // Fallback: derive from the centralised map by
                    // name. Keeps the proxy compatible with daemon
                    // builds that pre-date the capabilities field.
                    name.as_str()
                        .map(cu_core::tool::default_capabilities_for)
                        .unwrap_or_default()
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect()
                });
            let mut entry = serde_json::json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
                "annotations": {
                    "readOnlyHint": read_only,
                    "destructiveHint": destructive,
                    "idempotentHint": idempotent,
                    "openWorldHint": open_world,
                },
            });
            entry["capabilities"] = serde_json::Value::Array(capabilities);
            entry
        })
        .collect();

    // `capability_version` and `schema_version` are passed through
    // when the daemon emits them; older daemons fall back to the
    // proxy's compiled-in `CAPABILITY_VERSION` so MCP clients always
    // see the envelope keys.
    let capability_version = result
        .get("capability_version")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String(cu_core::tool::CAPABILITY_VERSION.to_owned()));
    let schema_version = result
        .get("schema_version")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String("1".to_owned()));

    Ok(with_proxy_tools(serde_json::json!({
        "tools": mcp_tools,
        "capability_version": capability_version,
        "schema_version": schema_version,
    })))
}

async fn send_daemon_request_async(
    socket_path: &str,
    request: DaemonRequest,
    context: &str,
) -> Result<DaemonResponse, TransportFailure> {
    let socket = socket_path.to_owned();
    let blocking = tokio::task::spawn_blocking(move || send_request(&socket, &request)).await;
    match blocking {
        Err(error) => Err(TransportFailure {
            message: format!("internal join error {context}: {error}"),
            delivered: true,
        }),
        Ok(Err(error)) => Err(TransportFailure {
            message: format!("daemon transport error {context}: {error}"),
            delivered: !never_delivered(&error),
        }),
        Ok(Ok(response)) => Ok(response),
    }
}

/// A call that got no answer from the daemon.
struct TransportFailure {
    message: String,
    /// The request may have reached the daemon, so what it asked for may have run.
    delivered: bool,
}

/// The target guard on the proxy side: refuse before anything reaches the
/// daemon, whose build may predate the guard. The model-supplied private
/// scope argument is ignored; `allowed` comes from this process's env.
fn proxy_target_guard(
    name: &str,
    args: &serde_json::Value,
    allowed: &[String],
) -> Result<(), cu_core::protocol::ToolResult> {
    #[cfg(target_os = "macos")]
    {
        let mut args = args.clone();
        crate::scope_args::take_allowed(&mut args);
        platform_macos::target_guard::enforce_native(name, &args, allowed)
    }
}

/// A guard refusal in the daemon's response shape, so the MCP client sees the
/// same `isError` result the daemon's own guard returns.
fn refusal_daemon_response(refusal: cu_core::protocol::ToolResult) -> DaemonResponse {
    DaemonResponse {
        ok: false,
        result: serde_json::to_value(refusal).ok(),
        error: Some("target_not_allowed".to_owned()),
        exit_code: Some(1),
    }
}

async fn call_daemon_tool(
    socket_path: &str,
    session_id: &str,
    name: &str,
    mut args: serde_json::Value,
) -> Result<DaemonResponse, TransportFailure> {
    // The model never sets the target scope; this process's env does.
    let allowed_targets = cu_core::target_policy::allowed_from_env();
    if let Err(refusal) = proxy_target_guard(name, &args, &allowed_targets) {
        return Ok(refusal_daemon_response(refusal));
    }
    crate::scope_args::scope_target_args(&mut args, &allowed_targets);
    send_daemon_request_async(
        socket_path,
        DaemonRequest {
            method: "call".to_owned(),
            name: Some(name.to_owned()),
            args: Some(args),
            session_id: Some(session_id.to_owned()),
        },
        &format!("forwarding `{name}`"),
    )
    .await
}

/// Lazy proxy startup has two independent milestones: the daemon/reaper is
/// connected, and the onboarding grant wait has completed. Keeping them
/// separate ensures a prompt status call cannot accidentally waive onboarding
/// for the next driving action.
#[cfg(target_os = "macos")]
#[derive(Debug, Default)]
struct DaemonStartState {
    reaper_started: bool,
    grant_wait_completed: bool,
    daemon_generation: u64,
    tools_list_generation: Option<u64>,
    authoritative_tools_list: Option<Arc<serde_json::Value>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DaemonLifecycleEvent {
    generation: u64,
}

#[cfg(target_os = "macos")]
impl DaemonStartState {
    fn needs_grant_wait(
        &self,
        request_requires_wait: bool,
        _external_permission_flow: bool,
    ) -> bool {
        request_requires_wait && !self.grant_wait_completed
    }

    fn complete_grant_wait(&mut self) {
        self.grant_wait_completed = true;
    }

    /// Record an observed socket outage. Until a replacement daemon is
    /// connected and queried, `tools/list` must fall back to the local,
    /// permission-free bootstrap registry rather than advertise the vanished
    /// daemon's contract.
    fn observe_daemon_loss(&mut self) {
        self.grant_wait_completed = false;
        self.tools_list_generation = None;
        self.authoritative_tools_list = None;
    }

    /// Apply the EOF signal from the generation-tagged control connection.
    /// Returns whether the event invalidated the current daemon. A delayed EOF
    /// from an older control task must not tear down a replacement generation.
    fn observe_control_connection_end(&mut self, generation: u64) -> bool {
        if !self.reaper_started || generation != self.daemon_generation {
            return false;
        }
        self.observe_daemon_loss();
        self.daemon_generation = self.daemon_generation.wrapping_add(1).max(1);
        true
    }

    /// Start a new observed daemon generation and return its stable token.
    /// The token lets the cache policy distinguish a replacement daemon from
    /// the one whose list it previously stored without a list round-trip on
    /// every tool call.
    fn begin_daemon_generation(&mut self) -> u64 {
        self.daemon_generation = self.daemon_generation.wrapping_add(1).max(1);
        self.reaper_started = true;
        self.daemon_generation
    }

    fn tools_list_refresh_generation(&self) -> Option<u64> {
        if self.reaper_started && self.tools_list_generation != Some(self.daemon_generation) {
            Some(self.daemon_generation)
        } else {
            None
        }
    }

    fn cache_authoritative_tools_list(
        &mut self,
        generation: u64,
        tools_list: Arc<serde_json::Value>,
    ) -> bool {
        if !self.reaper_started || generation != self.daemon_generation {
            return false;
        }
        self.authoritative_tools_list = Some(tools_list);
        self.tools_list_generation = Some(generation);
        true
    }

    fn tools_list_or<'a>(
        &'a self,
        bootstrap: &'a Arc<serde_json::Value>,
    ) -> &'a Arc<serde_json::Value> {
        match (
            self.tools_list_generation,
            self.authoritative_tools_list.as_ref(),
        ) {
            (Some(generation), Some(tools_list))
                if self.reaper_started && generation == self.daemon_generation =>
            {
                tools_list
            }
            _ => bootstrap,
        }
    }
}

/// `check_permissions` is itself the supported permission/status surface. It
/// must reach the daemon immediately, whether it is a read-only
/// `{ "prompt": false }` probe or the explicit prompting form, instead of
/// waiting up to 55 seconds for the condition it reports/requests. Every other
/// tool call retains the onboarding wait, including all driving actions.
#[cfg(target_os = "macos")]
fn tool_call_requires_grant_wait(req: &Request) -> bool {
    req.tool_call()
        .map(|call| call.name != "check_permissions")
        .unwrap_or(true)
}

/// Permission-free admission for macOS lazy proxy calls. Protocol shape, tool
/// identity, and the advertised input schema are checked before the helper is
/// started or health-recovered. The supplied list is the bootstrap registry
/// while dormant and the cached daemon-authored list after connection.
#[cfg(target_os = "macos")]
enum BootstrapToolCallAdmission {
    InvalidParams(String),
    Rejected(ToolResult),
    Ready {
        call: ToolCall,
        wait_for_grants: bool,
    },
}

#[cfg(target_os = "macos")]
fn admit_bootstrap_tool_call(
    req: &Request,
    tools_list: &serde_json::Value,
) -> BootstrapToolCallAdmission {
    let call = match req.tool_call() {
        Ok(call) => call,
        Err(error) => return BootstrapToolCallAdmission::InvalidParams(error.to_string()),
    };

    // Preserve the registry's sole deprecated alias even though aliases are
    // intentionally omitted from tools/list.
    let advertised_name = match call.name.as_str() {
        "type_text_chars" => "type_text",
        other => other,
    };
    let Some(entry) = tools_list
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .and_then(|tools| {
            tools.iter().find(|tool| {
                tool.get("name").and_then(serde_json::Value::as_str) == Some(advertised_name)
            })
        })
    else {
        return BootstrapToolCallAdmission::Rejected(ToolResult::error(format!(
            "Unknown tool: {}",
            call.name
        )));
    };

    let def = cu_core::tool::ToolDef {
        name: advertised_name.to_owned(),
        description: String::new(),
        input_schema: entry
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({ "type": "object" })),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: false,
    };
    if let Err(result) = crate::mcp::validate_dispatch_args(&def, &call.args) {
        return BootstrapToolCallAdmission::Rejected(result);
    }

    // Target guard before anything starts the helper or waits for grants:
    // a refused target is side-effect free. perform_actions is checked per
    // step; call_daemon_tool and the daemon's registry check again.
    let allowed_targets = cu_core::target_policy::allowed_from_env();
    let steps: Vec<(&str, &serde_json::Value)> = if call.name == PERFORM_ACTIONS_TOOL {
        call.args
            .get("actions")
            .and_then(serde_json::Value::as_array)
            .map(|actions| {
                actions
                    .iter()
                    .filter_map(|action| {
                        Some((action.get("tool")?.as_str()?, action.get("arguments")?))
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        vec![(call.name.as_str(), &call.args)]
    };
    for (tool, arguments) in steps {
        if let Err(refusal) = proxy_target_guard(tool, arguments, &allowed_targets) {
            return BootstrapToolCallAdmission::Rejected(refusal);
        }
    }

    let wait_for_grants = tool_call_requires_grant_wait(req);
    BootstrapToolCallAdmission::Ready {
        call,
        wait_for_grants,
    }
}

#[cfg(target_os = "macos")]
async fn ensure_daemon_started(
    socket_path: &str,
    state: &mut DaemonStartState,
    session_id: &str,
    claude_code_compat: bool,
    expected_profile: DaemonProfile,
    control_ready: &tokio::sync::watch::Sender<ControlConnectionState>,
    wait_for_grants: bool,
    lifecycle_tx: &tokio::sync::mpsc::UnboundedSender<DaemonLifecycleEvent>,
) -> anyhow::Result<()> {
    if !crate::supervisor::is_current(socket_path) {
        // A replacement daemon needs a fresh control connection and, for the
        // next driving action, a fresh onboarding stability wait.
        state.observe_daemon_loss();
        let sp = socket_path.to_owned();
        tokio::task::spawn_blocking(move || crate::supervisor::ensure(&sp, claude_code_compat))
            .await
            .map_err(|e| anyhow::anyhow!("daemon launch task failed: {e}"))??;
    }
    // Daemon is up: start the reaper now (deferred from proxy startup).
    if !state.reaper_started {
        let generation = state.begin_daemon_generation();
        let socket = socket_path.to_owned();
        let sid = session_id.to_owned();
        let ready = control_ready.clone();
        let lifecycle_tx = lifecycle_tx.clone();
        tokio::spawn(async move {
            run_control_connection(
                socket,
                sid,
                expected_profile,
                ready,
                Some((lifecycle_tx, generation)),
            )
            .await;
        });
    }
    wait_for_control_connection(control_ready.subscribe()).await?;
    // The bootstrap list intentionally keeps initialize/tools/list local and
    // permission-free while the lazy daemon is dormant. Once a daemon is
    // connected, however, that daemon is the execution authority and its
    // schema must win. Fetch once for this observed generation, then reuse the
    // cache until a socket outage invalidates it. `send_request` is blocking
    // UDS I/O, so keep it off Tokio's worker threads.
    if let Some(generation) = state.tools_list_refresh_generation() {
        let socket = socket_path.to_owned();
        let sid = session_id.to_owned();
        let tools_list = tokio::task::spawn_blocking(move || {
            fetch_tools_list_from_daemon(&socket, &sid, expected_profile)
        })
        .await
        .map_err(|e| anyhow::anyhow!("daemon tool-list task failed: {e}"))??;
        if !state.cache_authoritative_tools_list(generation, Arc::new(tools_list)) {
            anyhow::bail!("daemon changed while its authoritative tool list was loading");
        }
    }
    // Hold every driving call until the active permission authority reports the
    // complete flow ready. Host-owned onboarding additionally requires the
    // host's explicit post-verification readiness milestone.
    let external_permission_flow =
        crate::bundle::is_env_truthy("BRIGADIER_CU_EXTERNAL_PERMISSION_FLOW");
    if state.needs_grant_wait(wait_for_grants, external_permission_flow) {
        wait_for_daemon_permission_readiness(socket_path, session_id, external_permission_flow)
            .await?;
        state.complete_grant_wait();
    }
    Ok(())
}

/// Poll the daemon's private permission state until the complete authority is
/// ready. The bounded deadline fails closed instead of dispatching protected
/// work while onboarding is incomplete.
#[cfg(target_os = "macos")]
async fn wait_for_daemon_permission_readiness(
    socket_path: &str,
    session_id: &str,
    external_permission_flow: bool,
) -> anyhow::Result<()> {
    use std::time::{Duration, Instant};
    let started = Instant::now();
    // While a TCC grant is missing the user may be granting it right now, so
    // waiting is useful. When both grants are present and only the host's
    // readiness milestone is missing, waiting does not help: the host must
    // republish it, which needs a user action. Fail fast in that case.
    let grants_deadline = Duration::from_secs(55);
    // With Brigadier's setup window the user finishes setup there, which the
    // agent's call can wait for like a grant.
    let host_only_deadline = if external_permission_flow {
        grants_deadline
    } else {
        Duration::from_secs(6)
    };
    // Brigadier opens its setup window when asked; only once the helper says setup is
    // unfinished, so an agent arriving after setup never opens it.
    let mut setup_asked = !external_permission_flow;
    let mut last: Option<(bool, bool, Option<bool>)> = None;
    loop {
        let elapsed = started.elapsed();
        let host_only_missing = matches!(last, Some((true, true, host)) if host != Some(true));
        if elapsed >= grants_deadline || (host_only_missing && elapsed >= host_only_deadline) {
            let message = permission_readiness_failure(last, external_permission_flow);
            crate::journal::record_event("permission_gate_refused", &message);
            return Err(NotReady(message).into());
        }
        let sp = socket_path.to_owned();
        let sid = session_id.to_owned();
        let probe = tokio::task::spawn_blocking(move || {
            let req = DaemonRequest {
                method: "permissions_status".into(),
                name: None,
                args: None,
                session_id: Some(sid),
            };
            send_request(&sp, &req)
        })
        .await;
        if let Ok(Ok(resp)) = probe {
            if let Some(result) = resp.result.as_ref() {
                last = extract_permission_readiness(result);
                if daemon_permission_readiness(result, external_permission_flow) {
                    debug!(
                        external_permission_flow = external_permission_flow,
                        "daemon permission authority is ready"
                    );
                    return Ok(());
                }
                if !setup_asked {
                    crate::lifecycle::note_setup_needed(&crate::lifecycle::home());
                    setup_asked = true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
}

/// Permissions or setup aren't ready: the message already tells the agent
/// what is missing, so it reaches the agent as it is.
#[derive(Debug)]
struct NotReady(String);

impl std::fmt::Display for NotReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotReady {}

/// The one error an agent sees when the helper couldn't be brought back.
fn unavailable_message(error: &anyhow::Error) -> String {
    if let Some(not_ready) = error.downcast_ref::<NotReady>() {
        return not_ready.0.clone();
    }
    if crate::lifecycle::is_off(&crate::lifecycle::home()) {
        return crate::lifecycle::OFF_MESSAGE.to_owned();
    }
    format!(
        "Computer use isn't available right now: {error:#}. Brigadier tried to restart its \
         computer-use helper and couldn't. Tell the user if this keeps happening; don't retry \
         in a loop."
    )
}

/// Name exactly which precondition is missing so the agent (and the journal)
/// can tell a missing TCC grant from a host that has not republished readiness.
#[cfg(target_os = "macos")]
fn permission_readiness_failure(
    last: Option<(bool, bool, Option<bool>)>,
    external_permission_flow: bool,
) -> String {
    match last {
        None => {
            "Computer Use permissions are not ready: the Brigadier Computer Use helper did not \
                 answer a permission status request. Open Brigadier Settings > Computer Use."
                .to_owned()
        }
        Some((accessibility, screen_recording, _)) if !accessibility || !screen_recording => {
            let mut missing = Vec::new();
            if !accessibility {
                missing.push("Accessibility");
            }
            if !screen_recording {
                missing.push("Screen Recording");
            }
            format!(
                "Computer Use permissions are not ready: {} not granted to Brigadier Computer Use. \
                 Grant it in System Settings > Privacy & Security, or open Brigadier Settings > \
                 Computer Use, then retry.",
                missing.join(" and ")
            )
        }
        Some(_) if external_permission_flow => "Computer use setup isn't finished: Accessibility \
             and Screen Recording are granted, but screen capture hasn't been confirmed yet. Ask \
             the user to finish setup in the window Brigadier opened (or Settings > Computer use > \
             Finish setup…), then try again. This needs the user; don't retry in a loop."
            .to_owned(),
        Some(_) => "Computer Use permissions are not ready. Finish granting Accessibility and \
                    Screen Recording, then retry."
            .to_owned(),
    }
}

#[cfg(target_os = "macos")]
fn daemon_permission_readiness(value: &serde_json::Value, external_permission_flow: bool) -> bool {
    extract_permission_readiness(value).is_some_and(
        |(accessibility, screen_recording, host_ready)| {
            accessibility
                && screen_recording
                && (!external_permission_flow || host_ready == Some(true))
        },
    )
}

/// Recursively locate the permission payload without depending on the daemon's
/// result-envelope shape.
#[cfg(target_os = "macos")]
fn extract_permission_readiness(v: &serde_json::Value) -> Option<(bool, bool, Option<bool>)> {
    match v {
        serde_json::Value::Object(obj) => {
            if let (Some(a), Some(s)) = (
                obj.get("accessibility")
                    .and_then(serde_json::Value::as_bool),
                obj.get("screen_recording")
                    .and_then(serde_json::Value::as_bool),
            ) {
                return Some((
                    a,
                    s,
                    obj.get("external_permission_ready")
                        .and_then(serde_json::Value::as_bool),
                ));
            }
            obj.values().find_map(extract_permission_readiness)
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(extract_permission_readiness),
        _ => None,
    }
}

/// JSON-RPC method dispatcher for the proxy. Mirrors
/// `crate::mcp::handle_request`:
///   - `initialize`     → static `initialize_result()` (same envelope
///                        the in-process path returns; the daemon's
///                        identity is hidden from the MCP client).
///   - `tools/list`     → return the current bootstrap/daemon cache.
///   - `tools/call`     → forward to the daemon and reshape the
///                        response into MCP's `CallTool.Result`.
///   - other            → method-not-found, same as in-process.
async fn handle_proxy_request(
    req: Request,
    id: serde_json::Value,
    socket_path: &str,
    cached_tools_list: &serde_json::Value,
    session_id: &str,
    control_ready: &tokio::sync::watch::Receiver<ControlConnectionState>,
) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(
            id,
            crate::journal::with_complaint_instructions(initialize_result()),
        ),

        "tools/list" => Response::ok(id, cached_tools_list.clone()),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => {
                let read_only = tool_is_read_only(cached_tools_list, &call.name);
                match forward_tool_call(
                    id,
                    call.name,
                    call.args,
                    socket_path,
                    session_id,
                    control_ready,
                    read_only,
                )
                .await
                {
                    Forwarded::Done(response) | Forwarded::Lost(response) => response,
                }
            }
        },

        other => {
            warn!(method = other, "unknown method");
            Response::method_not_found(id, other)
        }
    }
}

#[cfg(target_os = "macos")]
fn response_from_tool_result(id: serde_json::Value, result: ToolResult) -> Response {
    match serde_json::to_value(result) {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::error(id, -32603, format!("Serialize error: {error}")),
    }
}

/// Forward a single MCP `tools/call` to the daemon as a `call`
/// request, then translate the `DaemonResponse` back into an MCP
/// `CallTool.Result` envelope.
///
/// Error mapping:
///   - Tool ran and reported failure (`!resp.ok`, including unknown
///     tool / bad params) → JSON-RPC success with `result.isError =
///     true`. Mirrors the in-process `crate::mcp` path so
///     MCP clients see identical envelopes either way.
///   - Transport failure (UDS unreachable, decode error, blocking
///     task panic) → JSON-RPC error (`-32603`), because the MCP
///     client really does need to distinguish "tool said no" from
///     "I couldn't reach the tool at all."
#[derive(Debug)]
struct PreparedProxyAction {
    tool: String,
    request: DaemonRequest,
}

#[derive(Debug)]
struct PreparedActionGroup {
    actions: Vec<PreparedProxyAction>,
    stop_on_error: bool,
}

fn prepare_action_group(
    args: serde_json::Value,
    session_id: &str,
    managed_session: bool,
) -> Result<PreparedActionGroup, String> {
    let allowed_targets = cu_core::target_policy::allowed_from_env();
    let object = args
        .as_object()
        .ok_or_else(|| "perform_actions arguments must be an object".to_owned())?;
    let actions = object
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "perform_actions requires an `actions` array".to_owned())?;
    if actions.is_empty() {
        return Err("perform_actions requires at least one action".to_owned());
    }
    if actions.len() > MAX_ACTION_GROUP_SIZE {
        return Err(format!(
            "perform_actions accepts at most {MAX_ACTION_GROUP_SIZE} actions"
        ));
    }
    let stop_on_error = object
        .get("stop_on_error")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);

    let mut prepared = Vec::with_capacity(actions.len());
    for (index, action) in actions.iter().enumerate() {
        let action = action
            .as_object()
            .ok_or_else(|| format!("perform_actions step {index} must be an object"))?;
        if let Some(field) = action
            .keys()
            .find(|field| field.as_str() != "tool" && field.as_str() != "arguments")
        {
            return Err(format!(
                "perform_actions step {index} has unknown field `{field}`"
            ));
        }
        let tool = action
            .get("tool")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("perform_actions step {index} requires string `tool`"))?;
        if !ACTION_GROUP_TOOLS.contains(&tool) {
            return Err(format!(
                "perform_actions step {index} cannot call `{tool}`; group only \
                 already-grounded input actions"
            ));
        }
        let arguments = action
            .get("arguments")
            .cloned()
            .filter(serde_json::Value::is_object)
            .ok_or_else(|| format!("perform_actions step {index} requires object `arguments`"))?;
        let mut arguments = enforce_proxy_session_identity(arguments, session_id, managed_session);
        if let Err(refusal) = proxy_target_guard(tool, &arguments, &allowed_targets) {
            let text = refusal
                .content
                .iter()
                .find_map(|content| match content {
                    cu_core::protocol::Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| "target_not_allowed".to_owned());
            return Err(format!("perform_actions step {index}: {text}"));
        }
        crate::scope_args::scope_target_args(&mut arguments, &allowed_targets);
        prepared.push(PreparedProxyAction {
            tool: tool.to_owned(),
            request: DaemonRequest {
                method: "call".into(),
                name: Some(tool.to_owned()),
                args: Some(arguments),
                session_id: Some(session_id.to_owned()),
            },
        });
    }

    Ok(PreparedActionGroup {
        actions: prepared,
        stop_on_error,
    })
}

fn action_group_error_response(
    id: serde_json::Value,
    message: String,
    requested_count: usize,
    results: Vec<serde_json::Value>,
) -> Response {
    let succeeded_count = results
        .iter()
        .filter(|result| result.get("isError") == Some(&serde_json::Value::Bool(false)))
        .count();
    Response::ok(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
            "structuredContent": {
                "requested_count": requested_count,
                "attempted_count": results.len(),
                "succeeded_count": succeeded_count,
                "results": results,
            },
        }),
    )
}

async fn forward_action_group(
    id: serde_json::Value,
    args: serde_json::Value,
    socket_path: &str,
    session_id: &str,
) -> Forwarded {
    let managed_session = configured_default_session()
        .is_some_and(|base| session_id.starts_with(&format!("{base}-mcp-")));
    let prepared = match prepare_action_group(args, session_id, managed_session) {
        Ok(prepared) => prepared,
        Err(message) => {
            return Forwarded::Done(action_group_error_response(id, message, 0, Vec::new()));
        }
    };
    let requested_count = prepared.actions.len();
    let socket = socket_path.to_owned();
    let blocking = tokio::task::spawn_blocking(move || {
        let mut results = Vec::with_capacity(requested_count);
        let mut any_error = false;
        for (index, action) in prepared.actions.into_iter().enumerate() {
            let response = match send_request(&socket, &action.request) {
                Ok(response) => response,
                Err(error) => {
                    // Nothing ran yet only if the first step never reached the daemon.
                    let nothing_ran = results.is_empty() && never_delivered(&error);
                    return Err((
                        format!(
                            "Computer Use transport failed at perform_actions step {index} \
                             (`{}`): {error}",
                            action.tool
                        ),
                        results,
                        nothing_ran,
                    ));
                }
            };

            let transport_ok = response.ok;
            let result = if transport_ok {
                response
                    .result
                    .unwrap_or_else(|| serde_json::json!({ "content": [], "isError": false }))
            } else {
                serde_json::json!({
                    "content": [{
                        "type": "text",
                        "text": response.error.unwrap_or_else(|| "daemon reported failure".into()),
                    }],
                    "isError": true,
                    "structuredContent": {
                        "exit_code": response.exit_code.unwrap_or(1),
                    },
                })
            };
            let is_error = !transport_ok
                || result
                    .get("isError")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
            results.push(serde_json::json!({
                "index": index,
                "tool": action.tool,
                "isError": is_error,
                "result": result,
            }));
            any_error |= is_error;
            if is_error && prepared.stop_on_error {
                break;
            }
        }
        Ok((results, any_error))
    })
    .await;

    let (results, any_error) = match blocking {
        Err(error) => {
            return Forwarded::Done(Response::error(
                id,
                -32603,
                format!("internal join error forwarding perform_actions: {error}"),
            ));
        }
        // Nothing ran yet, so the whole group can be retried safely.
        Ok(Err((message, results, true))) => {
            return Forwarded::Lost(action_group_error_response(
                id,
                message,
                requested_count,
                results,
            ));
        }
        Ok(Err((message, results, false))) => {
            return Forwarded::Done(action_group_error_response(
                id,
                message,
                requested_count,
                results,
            ));
        }
        Ok(Ok(outcome)) => outcome,
    };

    let succeeded_count = results
        .iter()
        .filter(|result| result.get("isError") == Some(&serde_json::Value::Bool(false)))
        .count();
    let attempted_count = results.len();
    let summary = if any_error {
        format!(
            "perform_actions attempted {attempted_count} of {requested_count} actions; \
             {succeeded_count} succeeded."
        )
    } else {
        format!("Completed all {requested_count} Computer Use actions in one ordered call.")
    };
    Forwarded::Done(Response::ok(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": summary }],
            "isError": any_error,
            "structuredContent": {
                "requested_count": requested_count,
                "attempted_count": attempted_count,
                "succeeded_count": succeeded_count,
                "results": results,
            },
        }),
    ))
}

/// The outcome of forwarding one tool call to the daemon.
enum Forwarded {
    /// The daemon handled the call (successfully or not).
    Done(Response),
    /// The daemon went away and sending the call again is safe: it never
    /// reached the daemon, or it only reads. The response describes the
    /// failure if a retry is impossible.
    Lost(Response),
}

async fn forward_tool_call(
    id: serde_json::Value,
    name: String,
    args: serde_json::Value,
    socket_path: &str,
    session_id: &str,
    control_ready: &tokio::sync::watch::Receiver<ControlConnectionState>,
    read_only: bool,
) -> Forwarded {
    if let Err(error) = wait_for_control_connection(control_ready.clone()).await {
        return Forwarded::Lost(Response::error(
            id,
            -32603,
            format!(
                "daemon session control connection unavailable before forwarding `{name}`: {error}"
            ),
        ));
    }
    if name == PERFORM_ACTIONS_TOOL {
        return forward_action_group(id, args, socket_path, session_id).await;
    }
    let managed_session = configured_default_session()
        .is_some_and(|base| session_id.starts_with(&format!("{base}-mcp-")));
    let args = enforce_proxy_session_identity(args, session_id, managed_session);
    match call_daemon_tool(socket_path, session_id, &name, args).await {
        Ok(response) => Forwarded::Done(daemon_response_to_mcp(id, &name, response)),
        Err(failure) if read_only || !failure.delivered => {
            Forwarded::Lost(Response::error(id, -32603, failure.message))
        }
        // Sending it again could repeat an action that already happened.
        Err(failure) => Forwarded::Done(Response::error(
            id,
            -32603,
            format!(
                "Brigadier Computer Use stopped while running `{name}`, so it may or may not \
                 have taken effect. Take a fresh get_window_state before trying it again; the \
                 next call starts the helper again. ({})",
                failure.message
            ),
        )),
    }
}

/// Whether `tools_list` marks `name` read-only, so repeating it changes nothing.
fn tool_is_read_only(tools_list: &serde_json::Value, name: &str) -> bool {
    tools_list
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .and_then(|tools| {
            tools
                .iter()
                .find(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some(name))
        })
        .and_then(|tool| tool.pointer("/annotations/readOnlyHint"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn daemon_response_to_mcp(
    id: serde_json::Value,
    _name: &str,
    response: DaemonResponse,
) -> Response {
    if !response.ok {
        // MCP separates two failure modes:
        //   - JSON-RPC errors → `Response::error(...)`, used for
        //     transport / protocol failures (unknown method, bad
        //     params shape, server crash).
        //   - Tool-level errors → `Response::ok(...)` carrying a
        //     `CallTool.Result` with `isError: true` and the error
        //     message in `content[]`. The tool ran, returned a
        //     well-formed result that says "I failed."
        //
        // A non-`ok` daemon response means the tool call reached the
        // daemon and the daemon decided the tool returned an error
        // (or rejected the call). That's tool-level, not transport-
        // level, so the in-process `crate::mcp` would surface
        // it as `Response::ok` with `isError: true`. Mirror that
        // shape here so MCP clients see identical envelopes either
        // way — CodeRabbit #2.
        if let Some(result) = response
            .result
            .filter(|result| result.get("isError").and_then(|value| value.as_bool()) == Some(true))
        {
            return Response::ok(id, result);
        }
        let msg = response
            .error
            .unwrap_or_else(|| "daemon reported failure".into());
        let exit_code = response.exit_code.unwrap_or(1);
        let result = serde_json::json!({
            "content": [{ "type": "text", "text": msg }],
            "isError": true,
            "structuredContent": { "exit_code": exit_code }
        });
        return Response::ok(id, result);
    }

    let result = response.result.unwrap_or_else(|| {
        serde_json::json!({
            "content": [],
            "isError": false
        })
    });
    Response::ok(id, result)
}

fn configured_default_session() -> Option<String> {
    std::env::var(crate::env::DEFAULT_SESSION_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn configured_state_owner_pid() -> Option<u32> {
    std::env::var("BRIGADIER_CU_STATE_OWNER_PID")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| *value > 1)
}

fn managed_host_session(session_id: &str) -> Option<&str> {
    session_id
        .rfind("-mcp-")
        .map(|marker| &session_id[..marker])
        .filter(|value| !value.is_empty())
}

/// Binds one proxy connection to the embedding host's stable session while
/// preserving a unique MCP-generation suffix for concurrent/restarted agents.
fn proxy_session_identity(
    minted_session_id: &str,
    configured_default_session: Option<&str>,
) -> (String, bool) {
    match configured_default_session
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(default_session) => (format!("{default_session}-{minted_session_id}"), true),
        None => (minted_session_id.to_owned(), false),
    }
}

/// A brigadier-managed proxy owns its surface identity. Agent-supplied session
/// labels are useful for standalone CU runs, but must not detach hosted
/// activity from the menu/status/cleanup authority that launched the proxy.
fn enforce_proxy_session_identity(
    mut args: serde_json::Value,
    session_id: &str,
    managed_session: bool,
) -> serde_json::Value {
    if managed_session {
        if let Some(object) = args.as_object_mut() {
            let lifecycle_session = serde_json::Value::String(session_id.to_owned());
            object.insert("session".to_owned(), lifecycle_session.clone());
            object.insert("_session_id".to_owned(), lifecycle_session);
            if let Some(host_session) = managed_host_session(session_id) {
                object.insert(
                    crate::env::HOST_SESSION_ARG.to_owned(),
                    serde_json::Value::String(host_session.to_owned()),
                );
            }
            if let Some(owner_pid) = configured_state_owner_pid() {
                object.insert(
                    cu_core::session_state::STATE_OWNER_PID_ARG.to_owned(),
                    serde_json::json!(owner_pid),
                );
            }
        }
    }
    args
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// Unit-test only the JSON shape of the proxy's tool-error envelope.
// The full proxy loop is exercised by the macOS integration test
// (the BRIGADIER_CU_MCP_FORCE_PROXY harness); these tests just lock
// in the per-branch reshape so a
// regression to `Response::error` for tool-level failures would fail
// fast in CI on every platform.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_read_only_tool_counts_as_safe_to_repeat() {
        let tools = serde_json::json!({"tools": [
            {"name": "get_window_state", "annotations": {"readOnlyHint": true}},
            {"name": "type_text", "annotations": {"readOnlyHint": false}},
            {"name": "click"},
        ]});
        assert!(tool_is_read_only(&tools, "get_window_state"));
        assert!(!tool_is_read_only(&tools, "type_text"));
        assert!(!tool_is_read_only(&tools, "click"));
        assert!(!tool_is_read_only(&tools, "unknown"));
    }

    /// A request that reached the daemon may have run even when no answer came back; only one
    /// that never connected is known to have done nothing.
    #[cfg(unix)]
    #[test]
    fn a_request_counts_as_undelivered_only_when_it_never_connected() {
        use std::io::BufRead;
        let dir = std::env::temp_dir().join(format!("cu-proxy-delivery-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("d.sock");
        let socket_path = socket.to_str().unwrap().to_owned();
        let request = DaemonRequest {
            method: "call".into(),
            name: Some("type_text".into()),
            args: Some(serde_json::json!({})),
            session_id: None,
        };

        let error = send_request(&socket_path, &request).unwrap_err();
        assert!(never_delivered(&error), "{error}");

        // A daemon that reads the request and dies before answering.
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let daemon = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            line
        });
        let error = send_request(&socket_path, &request).unwrap_err();
        assert!(!never_delivered(&error), "{error}");
        assert!(daemon.join().unwrap().contains("type_text"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_proxy_refuses_a_guarded_target_before_forwarding() {
        // The driver's own pid is always refused; a model-supplied scope
        // argument does not unlock it.
        let own = std::process::id();
        let args = serde_json::json!({
            "pid": own,
            "_cu_allowed_target_bundle_ids": ["ai.brigadier.app"],
        });
        let refusal = proxy_target_guard("click", &args, &[]).unwrap_err();
        let response = refusal_daemon_response(refusal);
        assert!(!response.ok);
        let result = response.result.expect("typed refusal");
        assert_eq!(result["isError"], true);
        assert_eq!(result["structuredContent"]["code"], "target_not_allowed");
        assert!(proxy_target_guard("get_screen_size", &serde_json::json!({}), &[]).is_ok());
    }

    #[test]
    fn brigadier_default_session_scopes_the_proxy_connection() {
        assert_eq!(
            proxy_session_identity(
                "mcp-4242-99",
                Some("brigadier-C0D0FE9B-CB9C-421D-AD81-149B2318E7FF"),
            ),
            (
                "brigadier-C0D0FE9B-CB9C-421D-AD81-149B2318E7FF-mcp-4242-99".to_owned(),
                true,
            ),
        );
        assert_eq!(
            proxy_session_identity("mcp-4242-99", None),
            ("mcp-4242-99".to_owned(), false),
        );
    }

    #[test]
    fn brigadier_managed_proxy_rejects_agent_session_renaming() {
        let managed = "brigadier-C0D0FE9B-CB9C-421D-AD81-149B2318E7FF-mcp-4242-99";
        let args = enforce_proxy_session_identity(
            serde_json::json!({
                "session": "calculator-9-plus-10",
                "_session_id": "caller-spoofed",
                "pid": 84,
            }),
            managed,
            true,
        );

        assert_eq!(args["session"], managed);
        assert_eq!(args["_session_id"], managed);
        assert_eq!(
            args["_host_session"], "brigadier-C0D0FE9B-CB9C-421D-AD81-149B2318E7FF",
            "the cursor and menu state need a stable identity across MCP proxy generations"
        );
        assert_eq!(args["pid"], 84);
    }

    #[test]
    fn ordinary_proxy_preserves_explicit_session_identity() {
        let args = enforce_proxy_session_identity(
            serde_json::json!({
                "session": "research-1",
                "pid": 84,
            }),
            "mcp-4242-99",
            false,
        );

        assert_eq!(args["session"], "research-1");
        assert!(args.get("_session_id").is_none());
    }

    #[test]
    fn action_group_preserves_order_and_managed_cursor_identity() {
        let managed = "brigadier-C0D0FE9B-CB9C-421D-AD81-149B2318E7FF-mcp-4242-99";
        let group = prepare_action_group(
            serde_json::json!({
                "actions": [
                    {
                        "tool": "click",
                        "arguments": {
                            "pid": 84,
                            "window_id": 9,
                            "element_token": "first",
                            "session": "agent-override",
                        },
                    },
                    {
                        "tool": "press_key",
                        "arguments": {
                            "pid": 84,
                            "key": "return",
                        },
                    },
                ],
            }),
            managed,
            true,
        )
        .expect("valid action group");

        assert!(group.stop_on_error);
        assert_eq!(group.actions.len(), 2);
        assert_eq!(group.actions[0].tool, "click");
        assert_eq!(group.actions[1].tool, "press_key");
        for action in group.actions {
            assert_eq!(action.request.session_id.as_deref(), Some(managed));
            let arguments = action.request.args.expect("action arguments");
            assert_eq!(arguments["session"], managed);
            assert_eq!(arguments["_session_id"], managed);
        }
    }

    #[test]
    fn action_group_rejects_scans_and_unbounded_sequences() {
        let scan = prepare_action_group(
            serde_json::json!({
                "actions": [{
                    "tool": "get_window_state",
                    "arguments": { "pid": 84, "window_id": 9 },
                }],
            }),
            "mcp-1",
            false,
        )
        .expect_err("read/scanning tools must stay outside action groups");
        assert!(scan.contains("already-grounded input actions"));

        let actions: Vec<_> = (0..=MAX_ACTION_GROUP_SIZE)
            .map(|_| {
                serde_json::json!({
                    "tool": "press_key",
                    "arguments": { "pid": 84, "key": "return" },
                })
            })
            .collect();
        let oversized =
            prepare_action_group(serde_json::json!({ "actions": actions }), "mcp-1", false)
                .expect_err("action group must be bounded");
        assert!(oversized.contains("at most"));
    }

    /// Reconstruct the `!resp.ok` branch in isolation so we can assert
    /// on the serialized shape without spinning up a real daemon /
    /// tokio runtime. Keep this in sync with `forward_tool_call`.
    fn build_tool_error_response(id: serde_json::Value, resp: DaemonResponse) -> Response {
        let msg = resp
            .error
            .unwrap_or_else(|| "daemon reported failure".into());
        let exit_code = resp.exit_code.unwrap_or(1);
        let result = serde_json::json!({
            "content": [{ "type": "text", "text": msg }],
            "isError": true,
            "structuredContent": { "exit_code": exit_code }
        });
        Response::ok(id, result)
    }

    #[test]
    fn daemon_tool_failure_wraps_as_jsonrpc_success_with_iserror_true() {
        let daemon_resp = DaemonResponse {
            ok: false,
            result: None,
            error: Some("missing required field `pid`".into()),
            exit_code: Some(64),
        };
        let resp = build_tool_error_response(serde_json::json!(7), daemon_resp);
        let value = serde_json::to_value(&resp).expect("serialize");

        // Top-level JSON-RPC envelope: success (`result`), not error.
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], serde_json::json!(7));
        assert!(
            value.get("error").is_none(),
            "tool-level failure must NOT surface as JSON-RPC error: got {value}"
        );
        assert!(
            value.get("result").is_some(),
            "tool-level failure must carry a `result` payload: got {value}"
        );

        // CallTool.Result inside `result`: isError + content text.
        let result = &value["result"];
        assert_eq!(result["isError"], serde_json::json!(true));
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "missing required field `pid`");
        assert_eq!(result["structuredContent"]["exit_code"], 64);
    }

    #[test]
    fn daemon_failure_with_no_error_message_uses_fallback_text() {
        let daemon_resp = DaemonResponse {
            ok: false,
            result: None,
            error: None,
            exit_code: None,
        };
        let resp = build_tool_error_response(serde_json::json!("abc"), daemon_resp);
        let value = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(value["result"]["isError"], serde_json::json!(true));
        assert_eq!(
            value["result"]["content"][0]["text"],
            "daemon reported failure"
        );
        assert_eq!(value["result"]["structuredContent"]["exit_code"], 1);
    }

    #[test]
    fn daemon_tool_failure_preserves_original_structured_error() {
        let original = serde_json::json!({
            "content": [{"type":"text","text":"Call get_app_state first"}],
            "isError": true,
            "structuredContent": {
                "code": "no_active_app_session",
                "message": "Call get_app_state first"
            }
        });
        let daemon_resp = DaemonResponse::tool_error(original.clone(), "fallback", 1);
        let result = daemon_resp
            .result
            .filter(|result| result.get("isError").and_then(|value| value.as_bool()) == Some(true))
            .expect("structured tool error");
        assert_eq!(result, original);
    }

    fn daemon_list_result(profile: &str, names: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "profile": profile,
            "tools": names.iter().map(|name| serde_json::json!({"name": name})).collect::<Vec<_>>()
        })
    }

    #[test]
    fn daemon_profile_mismatch_fails_closed() {
        let other = daemon_list_result("other", &["click"]);
        let error =
            validate_daemon_profile_and_roster(&other, DaemonProfile::Native, "/tmp/explicit.sock")
                .expect_err("a daemon with another profile must not satisfy the proxy");
        assert!(error.to_string().contains("profile mismatch"));
        assert!(error.to_string().contains("/tmp/explicit.sock"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn control_connection_reconnects_after_daemon_rebind() {
        use tokio::net::{UnixListener, UnixStream};

        async fn accept_session_begin(
            listener: &UnixListener,
            expected_session: &str,
        ) -> UnixStream {
            let (mut stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept())
                    .await
                    .expect("control connection timeout")
                    .expect("accept control connection");
            let mut line = String::new();
            {
                let mut reader = BufReader::new(&mut stream);
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    reader.read_line(&mut line),
                )
                .await
                .expect("session_begin timeout")
                .expect("read session_begin");
            }
            let request: DaemonRequest =
                serde_json::from_str(line.trim()).expect("decode session_begin");
            assert_eq!(request.method, "session_begin");
            assert_eq!(request.session_id.as_deref(), Some(expected_session));
            let ack = DaemonResponse::ok(serde_json::json!({
                "session_begin": true,
                "profile": DaemonProfile::Native,
            }));
            stream
                .write_all(format!("{}\n", serde_json::to_string(&ack).unwrap()).as_bytes())
                .await
                .expect("write session_begin ACK");
            stream.flush().await.expect("flush session_begin ACK");
            stream
        }

        let root = tempfile::tempdir().expect("socket tempdir");
        let socket = root.path().join("daemon.sock");
        let first_listener = UnixListener::bind(&socket).expect("bind first daemon");
        let session_id = "proxy-reexec-test-session".to_owned();
        let (ready_tx, mut ready_rx) =
            tokio::sync::watch::channel(ControlConnectionState::Connecting);
        let task = tokio::spawn(run_control_connection(
            socket.to_string_lossy().into_owned(),
            session_id.clone(),
            DaemonProfile::Native,
            ready_tx,
            None,
        ));

        let first_stream = accept_session_begin(&first_listener, &session_id).await;
        wait_for_control_connection(ready_rx.clone())
            .await
            .expect("first control connection ready");
        drop(first_listener);
        drop(first_stream);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while matches!(&*ready_rx.borrow(), ControlConnectionState::Ready { .. }) {
                ready_rx.changed().await.expect("readiness sender alive");
            }
        })
        .await
        .expect("first control connection closes");
        std::fs::remove_file(&socket).expect("remove first daemon socket");

        let second_listener = UnixListener::bind(&socket).expect("bind replacement daemon");
        let second_stream = accept_session_begin(&second_listener, &session_id).await;
        wait_for_control_connection(ready_rx.clone())
            .await
            .expect("replacement control connection ready");

        task.abort();
        let _ = task.await;
        drop(second_stream);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bootstrap_admission_rejects_malformed_unknown_and_invalid_calls() {
        fn request(params: serde_json::Value) -> Request {
            serde_json::from_value(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": params,
            }))
            .expect("request envelope")
        }

        let tools_list = crate::build_macos_registry_with_compat(false).tools_list();

        let malformed = request(serde_json::json!({ "arguments": {} }));
        assert!(matches!(
            admit_bootstrap_tool_call(&malformed, &tools_list),
            BootstrapToolCallAdmission::InvalidParams(_)
        ));

        let unknown = request(serde_json::json!({
            "name": "definitely_not_a_tool",
            "arguments": {},
        }));
        match admit_bootstrap_tool_call(&unknown, &tools_list) {
            BootstrapToolCallAdmission::Rejected(result) => {
                let value = serde_json::to_value(result).expect("serialize rejection");
                assert_eq!(value["isError"], true);
                assert_eq!(
                    value["content"][0]["text"],
                    "Unknown tool: definitely_not_a_tool"
                );
            }
            _ => panic!("unknown tools must be rejected before daemon startup"),
        }

        let invalid = request(serde_json::json!({
            "name": "set_agent_cursor_enabled",
            "arguments": {},
        }));
        assert!(matches!(
            admit_bootstrap_tool_call(&invalid, &tools_list),
            BootstrapToolCallAdmission::Rejected(_)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bootstrap_admission_refuses_a_guarded_target_before_the_helper_starts() {
        // A refused target must not start the helper or wait for permission
        // grants: admission rejects it with the typed refusal.
        fn request(name: &str, arguments: serde_json::Value) -> Request {
            serde_json::from_value(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            }))
            .expect("request envelope")
        }
        let tools_list =
            with_proxy_tools(crate::build_macos_registry_with_compat(false).tools_list());
        let own = std::process::id();
        for req in [
            request(
                "get_window_state",
                serde_json::json!({ "pid": own, "window_id": 1 }),
            ),
            request(
                "perform_actions",
                serde_json::json!({ "actions": [
                    { "tool": "press_key", "arguments": { "pid": own, "key": "a" } }
                ] }),
            ),
        ] {
            match admit_bootstrap_tool_call(&req, &tools_list) {
                BootstrapToolCallAdmission::Rejected(result) => {
                    let text = serde_json::to_string(&result).unwrap();
                    assert!(text.contains("target_not_allowed"), "{text}");
                }
                _ => panic!("a guarded target must be rejected at admission"),
            }
        }
        assert!(matches!(
            admit_bootstrap_tool_call(
                &request("get_screen_size", serde_json::json!({})),
                &tools_list
            ),
            BootstrapToolCallAdmission::Ready { .. }
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bootstrap_admission_accepts_only_real_calls_and_preserves_wait_policy() {
        fn request(name: &str, arguments: serde_json::Value) -> Request {
            serde_json::from_value(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            }))
            .expect("request envelope")
        }

        let tools_list = crate::build_macos_registry_with_compat(false).tools_list();

        match admit_bootstrap_tool_call(
            &request("check_permissions", serde_json::json!({ "prompt": false })),
            &tools_list,
        ) {
            BootstrapToolCallAdmission::Ready {
                wait_for_grants, ..
            } => {
                assert!(!wait_for_grants, "permission status must remain prompt");
            }
            _ => panic!("check_permissions must be admitted"),
        }

        match admit_bootstrap_tool_call(
            &request(
                "set_agent_cursor_enabled",
                serde_json::json!({ "enabled": true }),
            ),
            &tools_list,
        ) {
            BootstrapToolCallAdmission::Ready {
                wait_for_grants, ..
            } => {
                assert!(wait_for_grants, "ordinary tools retain the onboarding wait");
            }
            _ => panic!("a schema-valid tool call must be admitted"),
        }

        assert!(matches!(
            admit_bootstrap_tool_call(
                &request(
                    "type_text_chars",
                    serde_json::json!({ "pid": 1, "text": "a" }),
                ),
                &tools_list,
            ),
            BootstrapToolCallAdmission::Ready { .. }
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn grant_wait_policy_bypasses_only_permission_status_calls() {
        fn request(name: &str, arguments: serde_json::Value) -> Request {
            serde_json::from_value(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments }
            }))
            .expect("valid request")
        }

        assert!(!tool_call_requires_grant_wait(&request(
            "check_permissions",
            serde_json::json!({ "prompt": false }),
        )));
        assert!(!tool_call_requires_grant_wait(&request(
            "check_permissions",
            serde_json::json!({ "prompt": true }),
        )));

        for driving_tool in ["click", "move_cursor", "type_text", "scroll"] {
            assert!(
                tool_call_requires_grant_wait(&request(driving_tool, serde_json::json!({}))),
                "{driving_tool} must retain the onboarding grant wait"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn permission_status_skip_does_not_waive_later_driving_wait() {
        let mut state = DaemonStartState {
            reaper_started: true,
            grant_wait_completed: false,
            ..DaemonStartState::default()
        };

        assert!(
            !state.needs_grant_wait(false, false),
            "the first permission status call must be forwarded promptly"
        );
        assert!(
            !state.grant_wait_completed,
            "skipping the status call must leave the grant milestone pending"
        );

        assert!(
            state.needs_grant_wait(true, false),
            "the next driving call must still perform the grant wait"
        );
        state.complete_grant_wait();
        assert!(state.grant_wait_completed);
        assert!(
            !state.needs_grant_wait(true, false),
            "a completed wait should not repeat while the daemon remains healthy"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn external_permission_flow_waits_for_host_readiness() {
        let state = DaemonStartState {
            reaper_started: true,
            grant_wait_completed: false,
            ..DaemonStartState::default()
        };

        assert!(
            state.needs_grant_wait(true, true),
            "host-owned onboarding must gate driving calls until the host publishes readiness"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn external_permission_readiness_requires_the_host_milestone() {
        let granted_without_host = serde_json::json!({
            "accessibility": true,
            "screen_recording": true,
            "external_permission_ready": false,
        });
        assert!(daemon_permission_readiness(&granted_without_host, false));
        assert!(!daemon_permission_readiness(&granted_without_host, true));

        let host_ready = serde_json::json!({
            "result": {
                "accessibility": true,
                "screen_recording": true,
                "external_permission_ready": true,
            }
        });
        assert!(daemon_permission_readiness(&host_ready, true));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tool_list_cache_tracks_observed_daemon_generations() {
        let bootstrap = Arc::new(serde_json::json!({ "source": "bootstrap" }));
        let daemon_v1 = Arc::new(serde_json::json!({ "source": "daemon-v1" }));
        let daemon_v2 = Arc::new(serde_json::json!({ "source": "daemon-v2" }));
        let mut state = DaemonStartState::default();

        assert_eq!(state.tools_list_or(&bootstrap)["source"], "bootstrap");
        assert_eq!(state.tools_list_refresh_generation(), None);

        let first_generation = state.begin_daemon_generation();
        assert_eq!(
            state.tools_list_refresh_generation(),
            Some(first_generation),
            "a newly connected daemon needs exactly one authoritative list fetch"
        );
        assert!(state.cache_authoritative_tools_list(first_generation, daemon_v1));
        assert_eq!(state.tools_list_or(&bootstrap)["source"], "daemon-v1");
        assert_eq!(
            state.tools_list_refresh_generation(),
            None,
            "healthy calls must reuse the generation cache instead of round-tripping"
        );

        assert!(state.observe_control_connection_end(first_generation));
        assert_eq!(
            state.tools_list_or(&bootstrap)["source"],
            "bootstrap",
            "an observed outage must stop advertising the vanished daemon's schema"
        );
        assert_eq!(
            state.tools_list_refresh_generation(),
            Some(state.daemon_generation)
        );

        let second_generation = state.daemon_generation;
        assert_ne!(second_generation, first_generation);
        assert_eq!(
            state.tools_list_refresh_generation(),
            Some(second_generation),
            "a replacement daemon must refresh the authoritative contract"
        );
        assert!(state.cache_authoritative_tools_list(second_generation, daemon_v2));
        assert_eq!(state.tools_list_or(&bootstrap)["source"], "daemon-v2");
        assert_eq!(state.tools_list_refresh_generation(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stale_daemon_generation_cannot_overwrite_replacement_tool_list() {
        let bootstrap = Arc::new(serde_json::json!({ "source": "bootstrap" }));
        let stale = Arc::new(serde_json::json!({ "source": "stale-daemon" }));
        let current = Arc::new(serde_json::json!({ "source": "current-daemon" }));
        let mut state = DaemonStartState::default();

        let stale_generation = state.begin_daemon_generation();
        assert!(state.observe_control_connection_end(stale_generation));
        let current_generation = state.daemon_generation;

        assert!(!state.cache_authoritative_tools_list(stale_generation, stale));
        assert_eq!(state.tools_list_or(&bootstrap)["source"], "bootstrap");
        assert!(state.cache_authoritative_tools_list(current_generation, current));
        assert_eq!(state.tools_list_or(&bootstrap)["source"], "current-daemon");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn control_eof_detects_replacement_completed_between_requests() {
        let bootstrap = Arc::new(serde_json::json!({ "source": "bootstrap" }));
        let daemon_v1 = Arc::new(serde_json::json!({ "source": "daemon-v1" }));
        let daemon_v2 = Arc::new(serde_json::json!({ "source": "daemon-v2" }));
        let mut state = DaemonStartState::default();

        let first_generation = state.begin_daemon_generation();
        assert!(state.cache_authoritative_tools_list(first_generation, daemon_v1));
        state.complete_grant_wait();
        assert!(state.reaper_started);
        assert!(state.grant_wait_completed);

        // The helper exits and its replacement binds the same path before the
        // next MCP request. Socket reachability alone is true both before and
        // after, but EOF from the old control connection identifies the loss.
        assert!(state.observe_control_connection_end(first_generation));
        assert!(
            state.reaper_started,
            "the reconnecting control supervisor remains the session owner"
        );
        assert!(!state.grant_wait_completed);
        assert_eq!(state.tools_list_generation, None);
        assert!(state.authoritative_tools_list.is_none());
        assert_eq!(state.tools_list_or(&bootstrap)["source"], "bootstrap");

        let replacement_generation = state.daemon_generation;
        assert_ne!(replacement_generation, first_generation);
        assert_eq!(
            state.tools_list_refresh_generation(),
            Some(replacement_generation)
        );
        assert!(state.cache_authoritative_tools_list(replacement_generation, daemon_v2));
        assert_eq!(state.tools_list_or(&bootstrap)["source"], "daemon-v2");

        assert!(
            !state.observe_control_connection_end(first_generation),
            "a delayed EOF from the old control task must not invalidate the replacement"
        );
        assert_eq!(state.tools_list_or(&bootstrap)["source"], "daemon-v2");
    }
}
