//! `brigadier mcp list | tools <server> | call <server> <tool> [<json>]` (D5): any MCP server of
//! the session's setup, on demand, the same way on every CLI, for its thread and every worker
//! (a worker's launch loads only its `--mcp` grants, D2). Brigadier's daemon is the client.
//!
//! One connection per caller and server is kept while it is used, so what a server holds for
//! its client (a page it opened, a handle it returned) stays good for the next call. It closes
//! after [`IDLE`] unused, or once its session or worker has ended; a gateway server's lease
//! goes with it.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use brigadier_cli::protocol::{CliReply, ErrorCode, ToolOutput};
use brigadier_providers::{McpServer, McpTransport};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::SessionManager;
use super::cli::CliCaller;
use super::mcp_gateway::SessionLease;
use crate::model::{ConversationId, TaskId};
use crate::tools::Role;

/// A kept connection unused this long closes.
pub(super) const IDLE: Duration = Duration::from_secs(10 * 60);
/// How often kept connections are checked.
const SWEEP: Duration = Duration::from_secs(30);
/// A server's start and handshake (`npx` may download it first).
const START: Duration = Duration::from_secs(60);
/// A tool call, unless the server's config names its own.
const CALL: Duration = Duration::from_secs(10 * 60);
/// The one reply frame that carries an answer to `brigadier` (`brigadier_ipc::MAX_FRAME_BYTES`).
const MAX_REPLY: usize = 16 * 1024 * 1024;
const MAX_RESPONSE: usize = MAX_REPLY;
const MAX_PAGES: usize = 32;
const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    conversation: ConversationId,
    task: Option<TaskId>,
    server: String,
}

type Slot = Arc<tokio::sync::Mutex<Option<Conn>>>;

/// The kept connections, by caller and server.
#[derive(Default)]
pub(super) struct McpPool {
    slots: Mutex<HashMap<Key, Slot>>,
    sweeping: AtomicBool,
}

impl McpPool {
    fn slots(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Slot>> {
        self.slots.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `key`'s connection, locked: never one the sweep dropped while this waited for it.
    async fn hold(&self, key: &Key) -> tokio::sync::OwnedMutexGuard<Option<Conn>> {
        loop {
            let slot = self.slots().entry(key.clone()).or_default().clone();
            let kept = slot.clone().lock_owned().await;
            if self
                .slots()
                .get(key)
                .is_some_and(|now| Arc::ptr_eq(now, &slot))
            {
                return kept;
            }
        }
    }
}

/// Why a request failed.
enum Failure {
    /// The server answered an error: the connection stays good.
    Rpc(String),
    /// The connection is gone or out of step: it is dropped, and the next call starts afresh.
    Broken(String),
}

impl Failure {
    fn message(self) -> String {
        match self {
            Self::Rpc(message) | Self::Broken(message) => message,
        }
    }
}

enum Link {
    Stdio {
        child: Child,
        stdin: Option<ChildStdin>,
        stdout: BufReader<ChildStdout>,
    },
    Http {
        client: reqwest::Client,
        url: String,
        headers: Vec<(String, String)>,
        session: Option<String>,
    },
}

struct Conn {
    /// The server as the setup resolved it, before any lease: a changed config reconnects.
    server: McpServer,
    /// Ends a stdio server's whole process tree, whatever it started too.
    platform: Arc<dyn brigadier_sandbox::Platform>,
    link: Link,
    next: u64,
    version: Option<String>,
    used: Instant,
    /// A gateway server's loopback capability, released when the connection goes.
    lease: Option<Arc<SessionLease>>,
}

/// The same server as `kept` was opened for.
fn same(kept: &McpServer, now: &McpServer) -> bool {
    match (&kept.gateway, &now.gateway) {
        (Some(kept), Some(now)) => kept.resource == now.resource && kept.row_id == now.row_id,
        (None, None) => kept.transport == now.transport && kept.env == now.env,
        _ => false,
    }
}

impl Conn {
    async fn open(
        server: McpServer,
        platform: Arc<dyn brigadier_sandbox::Platform>,
        cwd: &Path,
        env: &[(String, String)],
        lease: Option<(Arc<SessionLease>, McpServer)>,
    ) -> Result<Self, String> {
        let (lease, leased) = match lease {
            Some((lease, leased)) => (Some(lease), leased),
            None => (None, server.clone()),
        };
        let link = match &leased.transport {
            McpTransport::Stdio {
                command,
                args,
                cwd: own,
            } => {
                let mut command_line = tokio::process::Command::new(command);
                command_line
                    .args(args)
                    .current_dir(own.as_deref().unwrap_or(cwd))
                    .env_clear()
                    .envs(env.iter().map(|(name, value)| (name, value)))
                    .envs(leased.env.iter().map(|(name, value)| (name, value)))
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                // Its own group, so what it leaves behind is ended with it.
                #[cfg(unix)]
                command_line.process_group(0);
                let mut child = command_line
                    .spawn()
                    .map_err(|err| format!("couldn't start {}: {err}", command.display()))?;
                let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                    return Err("the server's pipes didn't open".into());
                };
                Link::Stdio {
                    child,
                    stdin: Some(stdin),
                    stdout: BufReader::new(stdout),
                }
            }
            McpTransport::Http { url } => Link::Http {
                client: reqwest::Client::new(),
                url: url.clone(),
                headers: leased.env.clone(),
                session: None,
            },
        };
        let mut conn = Self {
            server,
            platform,
            link,
            next: 0,
            version: None,
            used: Instant::now(),
            lease,
        };
        let params = json!({
            "protocolVersion": VERSIONS[0],
            "capabilities": {},
            "clientInfo": {"name": "Brigadier", "version": env!("CARGO_PKG_VERSION")},
        });
        let initialized = conn
            .request("initialize", params, START)
            .await
            .map_err(|failure| format!("the server didn't start: {}", failure.message()))?;
        let version = initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|version| VERSIONS.contains(version))
            .ok_or_else(|| {
                format!(
                    "the server speaks MCP {}, which Brigadier doesn't",
                    initialized["protocolVersion"]
                )
            })?;
        conn.version = Some(version.to_owned());
        conn.notify("notifications/initialized")
            .await
            .map_err(Failure::message)?;
        Ok(conn)
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        within: Duration,
    ) -> Result<Value, Failure> {
        self.next += 1;
        let id = self.next;
        let message = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let answer = match tokio::time::timeout(within, self.send(message, Some(id))).await {
            Ok(answer) => answer?,
            Err(_) => {
                return Err(Failure::Broken(format!(
                    "no answer to {method} in {} s",
                    within.as_secs()
                )));
            }
        };
        self.used = Instant::now();
        let Some(answer) = answer else {
            return Err(Failure::Broken(format!("no answer to {method}")));
        };
        if let Some(error) = answer.get("error") {
            let mut text = format!(
                "the server answered error {}: {}",
                error["code"],
                error["message"].as_str().unwrap_or("(no message)")
            );
            if let Some(data) = error.get("data") {
                text.push_str(&format!("\n{data}"));
            }
            return Err(Failure::Rpc(text));
        }
        answer
            .get("result")
            .cloned()
            .ok_or_else(|| Failure::Broken(format!("an answer to {method} with no result")))
    }

    async fn notify(&mut self, method: &str) -> Result<(), Failure> {
        let message = json!({"jsonrpc": "2.0", "method": method});
        tokio::time::timeout(START, self.send(message, None))
            .await
            .map_err(|_| Failure::Broken(format!("{method} wasn't taken")))?
            .map(|_| ())
    }

    /// Sends `message`; with an `id`, waits for its answer.
    async fn send(&mut self, message: Value, id: Option<u64>) -> Result<Option<Value>, Failure> {
        let version = self.version.clone();
        match &mut self.link {
            Link::Stdio { stdin, stdout, .. } => {
                let Some(input) = stdin.as_mut() else {
                    return Err(Failure::Broken("the server is closed".into()));
                };
                let line = format!("{message}\n");
                input
                    .write_all(line.as_bytes())
                    .await
                    .map_err(|err| Failure::Broken(format!("the server's input closed: {err}")))?;
                input.flush().await.ok();
                let Some(id) = id else { return Ok(None) };
                loop {
                    let mut line = Vec::new();
                    let read = (&mut *stdout)
                        .take(MAX_RESPONSE as u64)
                        .read_until(b'\n', &mut line)
                        .await
                        .map_err(|err| {
                            Failure::Broken(format!("couldn't read the server: {err}"))
                        })?;
                    if read == 0 {
                        return Err(Failure::Broken("the server exited".into()));
                    }
                    if !line.ends_with(b"\n") && line.len() >= MAX_RESPONSE {
                        return Err(Failure::Broken("the server's answer is too big".into()));
                    }
                    let Ok(value) = serde_json::from_slice::<Value>(&line) else {
                        continue; // a server's stray log line
                    };
                    if value.get("method").is_some() {
                        // A request of the server's own: Brigadier offers none of its asks.
                        if let Some(asked) = value.get("id") {
                            let answer = if value["method"] == "ping" {
                                json!({"jsonrpc": "2.0", "id": asked, "result": {}})
                            } else {
                                json!({"jsonrpc": "2.0", "id": asked,
                                    "error": {"code": -32601, "message": "not supported"}})
                            };
                            input
                                .write_all(format!("{answer}\n").as_bytes())
                                .await
                                .map_err(|err| Failure::Broken(err.to_string()))?;
                        }
                        continue;
                    }
                    if value.get("id").and_then(Value::as_u64) == Some(id) {
                        return Ok(Some(value));
                    }
                }
            }
            Link::Http {
                client,
                url,
                headers,
                session,
            } => {
                let mut request = client
                    .post(url.as_str())
                    .header("accept", "application/json, text/event-stream")
                    .json(&message);
                for (name, value) in headers.iter() {
                    request = request.header(name.as_str(), value.as_str());
                }
                if let Some(session) = session.as_deref() {
                    request = request.header("mcp-session-id", session);
                }
                if let Some(version) = &version {
                    request = request.header("mcp-protocol-version", version);
                }
                let response = request
                    .send()
                    .await
                    .map_err(|err| Failure::Broken(format!("couldn't reach the server: {err}")))?;
                let status = response.status();
                if matches!(status.as_u16(), 401 | 403) {
                    return Err(Failure::Broken(format!(
                        "the server refused ({status}): it needs a sign-in; the user signs in \
                         from Brigadier's Plugins page"
                    )));
                }
                if !status.is_success() {
                    return Err(Failure::Broken(format!("the server answered {status}")));
                }
                if session.is_none() {
                    *session = response
                        .headers()
                        .get("mcp-session-id")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                }
                let Some(id) = id else { return Ok(None) };
                super::mcp_gateway::response_value(response, id, MAX_RESPONSE)
                    .await
                    .map(Some)
                    .map_err(|()| Failure::Broken("an answer Brigadier couldn't read".into()))
            }
        }
    }

    /// Ends the connection: a stdio server's input closes, then it is stopped; an HTTP
    /// session is ended; a lease is released.
    async fn close(mut self) {
        match &mut self.link {
            Link::Stdio { child, stdin, .. } => {
                drop(stdin.take());
                let pid = child.id();
                if tokio::time::timeout(Duration::from_secs(2), child.wait())
                    .await
                    .is_err()
                {
                    if let Some(pid) = pid {
                        let _ = self.platform.processes().kill_tree(pid);
                    }
                    let _ = child.kill().await;
                }
                if let Some(pid) = pid {
                    let _ = self.platform.processes().kill_group(pid);
                }
            }
            Link::Http {
                client,
                url,
                headers,
                session: Some(session),
            } => {
                let mut request = client
                    .delete(url.as_str())
                    .header("mcp-session-id", session.as_str())
                    .timeout(Duration::from_secs(3));
                for (name, value) in headers.iter() {
                    request = request.header(name.as_str(), value.as_str());
                }
                if let Some(version) = &self.version {
                    request = request.header("mcp-protocol-version", version.as_str());
                }
                let _ = request.send().await;
            }
            Link::Http { .. } => {}
        }
        if let Some(lease) = self.lease.take() {
            lease.release();
        }
    }
}

impl Drop for Conn {
    /// A connection dropped mid-call (its caller hung up) never went back to the pool: its
    /// server's tree ends here.
    fn drop(&mut self) {
        if let Link::Stdio { child, .. } = &self.link
            && let Some(pid) = child.id()
        {
            let _ = self.platform.processes().kill_tree(pid);
        }
    }
}

impl SessionManager {
    /// `brigadier mcp <action>`, for `caller`.
    pub(super) async fn cli_mcp(&self, caller: &CliCaller, action: &str, given: Value) -> CliReply {
        if !matches!(
            self.grants.resolve(&caller.grant),
            Some(Role::Orchestrator { .. } | Role::Worker { .. } | Role::Chat { .. })
        ) {
            return CliReply::error(
                ErrorCode::Refused,
                "refused: `brigadier mcp` is for a session's thread, a Chat and their workers",
            );
        }
        let (servers, granted, cwd) = match self.mcp_catalog(caller).await {
            Ok(catalog) => catalog,
            Err(err) => return CliReply::error(ErrorCode::Failed, err.to_string()),
        };
        let text = match action {
            "list" => Ok(list_text(&servers, granted.as_deref())),
            "tools" | "call" => {
                let name = given["server"].as_str().unwrap_or_default();
                let Some(server) = servers.iter().find(|server| server.name == name) else {
                    return CliReply::error(
                        ErrorCode::Failed,
                        format!(
                            "no MCP server {name:?} in this setup; `brigadier mcp list` lists them"
                        ),
                    );
                };
                let key = Key {
                    conversation: caller.conversation_id.clone(),
                    task: caller.task_id.clone(),
                    server: name.to_owned(),
                };
                return self.mcp_use(key, server.clone(), &cwd, action, given).await;
            }
            other => Err(format!("no action {other:?}")),
        };
        reply(action, text, Vec::new())
    }

    /// `tools` or `call` on `server`'s kept connection, opened first when there is none.
    async fn mcp_use(
        &self,
        key: Key,
        server: McpServer,
        cwd: &Path,
        action: &str,
        given: Value,
    ) -> CliReply {
        let mut kept = self.mcp_pool.hold(&key).await;
        self.sweep_mcp_soon();
        // Out of the pool while in use: a call cut off mid-exchange never leaves it half-written.
        let mut conn = match kept.take() {
            Some(conn) if same(&conn.server, &server) => conn,
            old => {
                if let Some(old) = old {
                    old.close().await;
                }
                let lease = lease(self.mcp_gateway(), &server);
                let env = self.clis().env;
                let platform = self.runtime.platform().clone();
                match Conn::open(server.clone(), platform, cwd, &env, lease).await {
                    Ok(conn) => conn,
                    Err(err) => return CliReply::error(ErrorCode::Failed, err),
                }
            }
        };
        let answer = match action {
            "tools" => list_tools(&mut conn, &server.name)
                .await
                .map(|text| (text, Vec::new())),
            _ => {
                let within = server.tool_timeout_secs.map_or(CALL, Duration::from_secs);
                let params = json!({"name": given["tool"], "arguments": given["arguments"]});
                match conn.request("tools/call", params, within).await {
                    Ok(result) => {
                        let (text, images) = result_text(&result);
                        if result["isError"] == true {
                            Err(Failure::Rpc(format!("the tool answered an error:\n{text}")))
                        } else {
                            Ok((text, images))
                        }
                    }
                    Err(failure) => Err(failure),
                }
            }
        };
        if let Err(Failure::Broken(_)) = &answer {
            conn.close().await;
        } else {
            *kept = Some(conn);
        }
        match answer {
            Ok((text, images)) => reply(action, Ok(text), images),
            Err(Failure::Rpc(message)) => CliReply::error(ErrorCode::Failed, message),
            Err(Failure::Broken(message)) => CliReply::error(
                ErrorCode::Failed,
                format!("{message}; the next call starts the server afresh"),
            ),
        }
    }

    /// Starts the check of kept connections, when it isn't running.
    fn sweep_mcp_soon(&self) {
        if self.mcp_pool.sweeping.swap(true, Ordering::AcqRel) {
            return;
        }
        let me = self.me.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SWEEP).await;
                let Some(manager) = me.upgrade() else { return };
                if manager.sweep_mcp().await == 0 {
                    manager.mcp_pool.sweeping.store(false, Ordering::Release);
                    // A call that kept a connection just now, while this was still sweeping,
                    // started no sweep of its own: this one goes on for it.
                    if manager.mcp_pool.slots().is_empty()
                        || manager.mcp_pool.sweeping.swap(true, Ordering::AcqRel)
                    {
                        return;
                    }
                }
            }
        });
    }

    /// Closes the kept connections unused for [`IDLE`] or whose session or worker has ended;
    /// returns how many are left.
    pub(super) async fn sweep_mcp(&self) -> usize {
        let slots: Vec<(Key, Slot)> = self
            .mcp_pool
            .slots()
            .iter()
            .map(|(key, slot)| (key.clone(), slot.clone()))
            .collect();
        for (key, slot) in slots {
            // In use: it is checked next time.
            let Ok(mut kept) = slot.try_lock() else {
                continue;
            };
            let live = match &key.task {
                Some(task) => self.existing_task_live(task).is_some(),
                None => self.convs_lock().contains_key(&key.conversation),
            };
            let idle = kept.as_ref().is_none_or(|conn| conn.used.elapsed() >= IDLE);
            if live && !idle {
                continue;
            }
            // Out of the map while still held: a caller waiting on it takes a fresh slot.
            self.mcp_pool.slots().remove(&key);
            let conn = kept.take();
            drop(kept);
            if let Some(conn) = conn {
                conn.close().await;
            }
        }
        self.mcp_pool.slots().len()
    }
}

/// A gateway server's own loopback capability, and the server as it reaches it.
fn lease(
    gateway: &Arc<super::mcp_gateway::McpGatewayService>,
    server: &McpServer,
) -> Option<(Arc<SessionLease>, McpServer)> {
    server.gateway.as_ref()?;
    let mut one = super::plugins::resolve::Resolved {
        servers: vec![server.clone()],
        ..Default::default()
    };
    let lease = gateway.lease_session(&mut one);
    Some((lease, one.servers.remove(0)))
}

fn reply(action: &str, text: Result<String, String>, images: Vec<String>) -> CliReply {
    match text {
        Ok(mut text) => {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let output = ToolOutput {
                action: action.to_owned(),
                text,
                images,
                saved: Vec::new(),
            };
            let size = serde_json::to_vec(&output).map_or(0, |bytes| bytes.len());
            // Room for the reply's own envelope.
            if size > MAX_REPLY - 64 * 1024 {
                return CliReply::error(
                    ErrorCode::Failed,
                    format!("the answer is {size} bytes, more than one reply carries"),
                );
            }
            CliReply::ok(&output)
        }
        Err(message) => CliReply::error(ErrorCode::Failed, message),
    }
}

fn list_text(servers: &[McpServer], granted: Option<&[String]>) -> String {
    if servers.is_empty() {
        return "No MCP servers in this setup.\n".into();
    }
    let mut text = format!("MCP servers ({}):\n", servers.len());
    for server in servers {
        let transport = match server.transport {
            McpTransport::Stdio { .. } => "stdio",
            McpTransport::Http { .. } => "HTTP",
        };
        let loaded = granted.is_none_or(|granted| granted.contains(&server.name));
        text.push_str(&format!(
            "- {} ({transport}){}\n",
            server.name,
            if loaded {
                ", loaded at your launch"
            } else {
                ""
            }
        ));
    }
    text.push_str(
        "\n`brigadier mcp tools <server>` lists a server's tools; `brigadier mcp call <server> \
         <tool> '<json>'` runs one.\n",
    );
    text
}

/// Every tool, page by page.
async fn list_tools(conn: &mut Conn, server: &str) -> Result<String, Failure> {
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..MAX_PAGES {
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |cursor| json!({"cursor": cursor}));
        let page = conn.request("tools/list", params, START).await?;
        tools.extend(page["tools"].as_array().cloned().unwrap_or_default());
        cursor = page["nextCursor"].as_str().map(str::to_owned);
        match &cursor {
            None => return Ok(tools_text(server, &tools)),
            Some(next) if !seen.insert(next.clone()) => {
                return Err(Failure::Rpc("the server's tool pages repeat".into()));
            }
            Some(_) => {}
        }
    }
    Err(Failure::Rpc(format!(
        "the server has more than {MAX_PAGES} pages of tools"
    )))
}

fn tools_text(server: &str, tools: &[Value]) -> String {
    if tools.is_empty() {
        return format!("{server} has no tools.\n");
    }
    let mut text = format!("{server}'s tools ({}):\n", tools.len());
    for tool in tools {
        text.push_str(&format!("\n- {}", tool["name"].as_str().unwrap_or("?")));
        if let Some(description) = tool["description"].as_str() {
            text.push_str(&format!(": {}", description.trim()));
        }
        text.push_str(&format!("\n  input: {}\n", tool["inputSchema"]));
        if let Some(output) = tool.get("outputSchema") {
            text.push_str(&format!("  output: {output}\n"));
        }
    }
    text.push_str(&format!(
        "\nRun one: `brigadier mcp call {server} <tool> '<json>'`.\n"
    ));
    text
}

/// A call's whole result: each content block, then its structured content; images also as
/// `data:` URLs, which `brigadier` saves to files.
fn result_text(result: &Value) -> (String, Vec<String>) {
    let mut text = String::new();
    let mut images = Vec::new();
    for block in result["content"].as_array().into_iter().flatten() {
        let mime = block["mimeType"].as_str().unwrap_or("?");
        match block["type"].as_str() {
            Some("text") => text.push_str(block["text"].as_str().unwrap_or_default()),
            // `brigadier` saves each to a file and prints its path.
            Some(kind @ ("image" | "audio")) => {
                images.push(format!(
                    "data:{mime};base64,{}",
                    block["data"].as_str().unwrap_or_default()
                ));
                text.push_str(&format!("[{kind} {mime}]"));
            }
            Some("resource") => match block["resource"]["text"].as_str() {
                Some(body) => text.push_str(&format!(
                    "[resource {}]\n{body}",
                    block["resource"]["uri"].as_str().unwrap_or("?")
                )),
                None => text.push_str(&format!("[resource] {}", block["resource"])),
            },
            _ => text.push_str(&block.to_string()),
        }
        text.push('\n');
    }
    if let Some(structured) = result.get("structuredContent") {
        text.push_str("structuredContent:\n");
        text.push_str(&serde_json::to_string_pretty(structured).unwrap_or_default());
        text.push('\n');
    }
    if text.is_empty() {
        text.push_str("(no content)\n");
    }
    (text, images)
}

#[cfg(test)]
#[path = "cli_mcp_tests.rs"]
pub(super) mod tests;
