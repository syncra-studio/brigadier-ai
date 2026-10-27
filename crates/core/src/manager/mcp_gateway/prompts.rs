//! Optional draft-menu enrichment. Reads return immediately; HTTP discovery is isolated
//! from CLI sessions, shares the gateway's refresh authority, and never initiates OAuth.
use super::McpGatewayService;
use brigadier_providers::{CliCommand, CliCommandKind, McpServer, ProviderKind};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::Ordering},
    time::{Duration, Instant},
};

const FRESH: Duration = Duration::from_secs(30);
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RESPONSE: usize = 1024 * 1024;
const MAX_PAGES: usize = 32;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    row: String,
    resource: String,
    grant: u64,
}
#[derive(Clone, Deserialize)]
struct Prompt {
    name: String,
    description: Option<String>,
    #[serde(default)]
    arguments: Vec<Argument>,
}
#[derive(Clone, Deserialize)]
struct Argument {
    name: String,
    #[serde(default)]
    required: bool,
}
struct Entry {
    prompts: Vec<Prompt>,
    attempted: Instant,
    refreshing: bool,
}
#[derive(Default)]
pub(super) struct Cache(Mutex<HashMap<Key, Entry>>);
impl Cache {
    pub(super) fn invalidate(&self, resource: &str) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|key, _| key.resource != resource);
    }
}

impl McpGatewayService {
    /// Only resolver-selected servers with this daemon's local capability are eligible.
    /// Cache identity uses the grant generation, so refresh rotations keep their cache while
    /// sign-out/new sign-in cannot revive prompts belonging to a previous grant.
    pub fn cached_prompts(self: &Arc<Self>, servers: &[McpServer]) -> Vec<CliCommand> {
        let mut commands = Vec::new();
        for server in servers {
            let Some(endpoint) = &server.gateway else {
                continue;
            };
            let key = {
                let entry = self.resource(&endpoint.resource);
                if !entry.granted.load(Ordering::Acquire) || entry.fallback.load(Ordering::Acquire)
                {
                    continue;
                }
                if !endpoint.secret_header.1.is_empty() {
                    let leases = self.leases.lock().unwrap_or_else(|p| p.into_inner());
                    if !leases.get(&endpoint.secret_header.1).is_some_and(|lease| {
                        lease.expires > Instant::now()
                            && lease.generation == entry.grant_generation.load(Ordering::Acquire)
                    }) {
                        continue;
                    }
                }
                Key {
                    row: endpoint.row_id.clone(),
                    resource: endpoint.resource.clone(),
                    grant: entry.grant_generation.load(Ordering::Acquire),
                }
            };
            let (prompts, refresh) = {
                let mut cache = self
                    .prompt_cache
                    .0
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                let entry = cache.entry(key.clone()).or_insert_with(|| Entry {
                    prompts: Vec::new(),
                    attempted: Instant::now() - FRESH,
                    refreshing: false,
                });
                let refresh = !entry.refreshing && entry.attempted.elapsed() >= FRESH;
                if refresh {
                    entry.refreshing = true;
                    entry.attempted = Instant::now();
                }
                (entry.prompts.clone(), refresh)
            };
            if refresh {
                let gateway = self.clone();
                let key = key.clone();
                tokio::spawn(async move {
                    let mut session = Session {
                        gateway: &gateway,
                        key: &key,
                        session: None,
                        version: None,
                    };
                    let result = tokio::time::timeout(DISCOVERY_TIMEOUT, session.list())
                        .await
                        .unwrap_or(Err(()));
                    session.close().await;
                    let resource = gateway.resource(&key.resource);
                    let valid = resource.granted.load(Ordering::Acquire)
                        && resource.grant_generation.load(Ordering::Acquire) == key.grant;
                    let mut cache = gateway
                        .prompt_cache
                        .0
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    if let Some(entry) = cache.get_mut(&key) {
                        entry.refreshing = false;
                        entry.attempted = Instant::now();
                        if valid && let Ok(prompts) = result {
                            entry.prompts = prompts;
                        }
                        if !valid {
                            cache.remove(&key);
                        }
                    }
                });
            }
            for prompt in prompts {
                let invocation = format!("mcp__{}__{}", server.name, prompt.name);
                let argument_hint = prompt
                    .arguments
                    .iter()
                    .map(|argument| {
                        if argument.required {
                            format!("<{}>", argument.name)
                        } else {
                            format!("[{}]", argument.name)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                commands.push(CliCommand {
                    name: prompt.name,
                    text: format!("/{invocation}"),
                    invocation: Some(invocation),
                    source: Some(server.name.clone()),
                    kind: Some(CliCommandKind::Prompt),
                    agent_only: Some(ProviderKind::Claude),
                    description: prompt.description,
                    argument_hint: (!argument_hint.is_empty()).then_some(argument_hint),
                });
            }
        }
        commands
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_prompt_cache(&self) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if !self
                    .prompt_cache
                    .0
                    .lock()
                    .unwrap()
                    .values()
                    .any(|entry| entry.refreshing)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(test)]
    pub(super) fn expire_prompt_cache(&self) {
        for entry in self.prompt_cache.0.lock().unwrap().values_mut() {
            entry.attempted = Instant::now() - FRESH;
        }
    }
}

struct Session<'a> {
    gateway: &'a McpGatewayService,
    key: &'a Key,
    session: Option<String>,
    version: Option<String>,
}
impl Session<'_> {
    fn valid(&self) -> bool {
        let resource = self.gateway.resource(&self.key.resource);
        resource.granted.load(Ordering::Acquire)
            && resource.grant_generation.load(Ordering::Acquire) == self.key.grant
    }
    async fn post(&mut self, message: Value) -> Result<Option<Value>, ()> {
        if !self.valid() {
            return Err(());
        }
        let (bearer, cancelled) = self
            .gateway
            .bearer(&self.key.resource)
            .await
            .map_err(|_| ())?;
        if !self.valid() {
            return Err(());
        }
        let mut request = self
            .gateway
            .client
            .post(&self.key.resource)
            .bearer_auth(&bearer)
            .header("accept", "application/json, text/event-stream")
            .timeout(DISCOVERY_TIMEOUT)
            .json(&message);
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        if let Some(version) = &self.version {
            request = request.header("mcp-protocol-version", version);
        }
        let response = tokio::select! {
            _ = cancelled.cancelled() => return Err(()),
            response = request.send() => response.map_err(|_| ())?,
        };
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.gateway
                .reject_bearer_for_grant(&self.key.resource, &bearer, Some(self.key.grant))
                .await;
            return Err(());
        }
        if !response.status().is_success() {
            return Err(());
        }
        if self.session.is_none() {
            self.session = response
                .headers()
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return Ok(None);
        };
        let value = tokio::select! {
            _ = cancelled.cancelled() => return Err(()),
            value = response_value(response, id) => value?,
        };
        if !self.valid() || value.get("error").is_some() {
            return Err(());
        }
        value.get("result").cloned().map(Some).ok_or(())
    }
    async fn list(&mut self) -> Result<Vec<Prompt>, ()> {
        let initialized = self.post(json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
            "protocolVersion":"2025-11-25", "capabilities":{}, "clientInfo":{"name":"Brigadier", "version":env!("CARGO_PKG_VERSION")}
        }})).await?.ok_or(())?;
        let version = initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or(())?;
        if !matches!(version, "2025-11-25" | "2025-06-18" | "2025-03-26") {
            return Err(());
        }
        self.version = Some(version.into());
        self.post(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}))
            .await?;
        if initialized.pointer("/capabilities/prompts").is_none() {
            return Ok(Vec::new());
        }
        let mut prompts = Vec::new();
        let mut cursor = None;
        let mut cursors = std::collections::HashSet::new();
        for page in 0..MAX_PAGES {
            let params = cursor
                .as_ref()
                .map_or_else(|| json!({}), |cursor: &String| json!({"cursor":cursor}));
            let result = self.post(json!({"jsonrpc":"2.0", "id":page + 2, "method":"prompts/list", "params":params})).await?.ok_or(())?;
            let listed: Vec<Prompt> =
                serde_json::from_value(result.get("prompts").cloned().ok_or(())?)
                    .map_err(|_| ())?;
            prompts.extend(listed.into_iter().filter(|prompt| !prompt.name.is_empty()));
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let Some(next) = &cursor else {
                return Ok(prompts);
            };
            if !cursors.insert(next.clone()) {
                return Err(());
            }
        }
        Err(())
    }
    async fn close(&self) {
        let Some(session) = &self.session else { return };
        if !self.valid() {
            return;
        }
        let Ok((bearer, _)) = self.gateway.bearer(&self.key.resource).await else {
            return;
        };
        if !self.valid() {
            return;
        }
        let mut request = self
            .gateway
            .client
            .delete(&self.key.resource)
            .bearer_auth(bearer)
            .header("mcp-session-id", session)
            .timeout(Duration::from_secs(3));
        if let Some(version) = &self.version {
            request = request.header("mcp-protocol-version", version);
        }
        let _ = request.send().await;
    }
}

/// Read JSON or SSE until this RPC's response, ignoring notifications and other event IDs.
/// Incremental line parsing handles CRLF, multiline data, and arbitrary HTTP chunk splits.
async fn response_value(mut response: reqwest::Response, id: u64) -> Result<Value, ()> {
    let sse = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|value| value.trim() == "text/event-stream")
        });
    let mut bytes = Vec::new();
    let mut event = Vec::new();
    let mut received = 0usize;
    while let Some(chunk) = response.chunk().await.map_err(|_| ())? {
        received = received.checked_add(chunk.len()).ok_or(())?;
        if received > MAX_RESPONSE {
            return Err(());
        }
        bytes.extend_from_slice(&chunk);
        if !sse {
            continue;
        }
        while let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            let mut line: Vec<_> = bytes.drain(..=newline).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.is_empty() {
                if let Ok(value) = serde_json::from_slice::<Value>(&event)
                    && value.get("id").and_then(Value::as_u64) == Some(id)
                {
                    return Ok(value);
                }
                event.clear();
            } else if let Some(data) = line.strip_prefix(b"data:") {
                let data = data.strip_prefix(b" ").unwrap_or(data);
                if !event.is_empty() {
                    event.push(b'\n');
                }
                event.extend_from_slice(data);
            }
        }
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| ())?;
    if !sse && value.get("id").and_then(Value::as_u64) == Some(id) {
        Ok(value)
    } else {
        Err(())
    }
}
