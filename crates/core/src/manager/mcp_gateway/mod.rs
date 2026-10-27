//! Daemon-owned streamable HTTP gateway. Every resolver call issues a distinct local
//! capability; upstream OAuth credentials never leave this module or the daemon's store.
mod manager;
mod oauth;
mod prompts;
pub mod store;
#[cfg(test)]
pub(super) mod tests;

use super::plugins::{CatalogSignIn, resolve::Gateway};
use axum::{
    Router,
    body::Body,
    extract::{Path as RoutePath, State},
    http::{Request, Response, StatusCode},
    routing::any,
};
use brigadier_providers::McpGateway;
use futures_util::StreamExt;
use std::{
    collections::HashMap,
    io,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use store::{Grant, TokenStore};
use tokio_util::sync::CancellationToken;

pub const SECRET_HEADER: &str = "x-brigadier-mcp-secret";

struct Credentials {
    grant: Option<Grant>,
    pending: Option<Grant>,
    state: CatalogSignIn,
    generation: u64,
    streams: CancellationToken,
}
struct Resource {
    credentials: tokio::sync::Mutex<Credentials>,
    granted: AtomicBool,
    grant_generation: AtomicU64,
    fallback: AtomicBool,
}
struct Lease {
    row: String,
    resource: String,
    generation: u64,
    owner: String,
    expires: std::time::Instant,
    cancelled: CancellationToken,
}

const LEASE_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

pub struct SessionLease {
    gateway: std::sync::Weak<McpGatewayService>,
    owner: String,
}
impl SessionLease {
    pub(super) fn id(&self) -> &str {
        &self.owner
    }
    pub fn release(&self) {
        if let Some(gateway) = self.gateway.upgrade() {
            gateway.release_leases(&self.owner);
        }
    }
}
impl Drop for SessionLease {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct McpGatewayService {
    address: String,
    client: reqwest::Client,
    store: Arc<dyn TokenStore>,
    resources: Mutex<HashMap<String, Arc<Resource>>>,
    leases: Mutex<HashMap<String, Lease>>,
    stop: CancellationToken,
    client_document: Option<String>,
    prompt_cache: prompts::Cache,
}
impl Drop for McpGatewayService {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl McpGatewayService {
    /// The listener owns only a Weak reference; dropping the manager stops the listener.
    pub fn new(data_dir: &Path) -> io::Result<Arc<Self>> {
        Self::with_store(
            Arc::new(store::KeychainStore::new(data_dir)),
            std::env::var("BRIGADIER_MCP_CLIENT_METADATA_URL").ok(),
        )
    }
    pub fn with_store(
        store: Arc<dyn TokenStore>,
        client_document: Option<String>,
    ) -> io::Result<Arc<Self>> {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = format!("http://{}", listener.local_addr()?);
        let listener = tokio::net::TcpListener::from_std(listener)?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(io::Error::other)?;
        let stop = CancellationToken::new();
        let gateway = Arc::new(Self {
            address,
            client,
            store,
            resources: Mutex::default(),
            leases: Mutex::default(),
            stop: stop.clone(),
            client_document,
            prompt_cache: prompts::Cache::default(),
        });
        let router = Router::new()
            .route("/mcp/{row}", any(proxy))
            .with_state(Arc::downgrade(&gateway));
        tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await;
        });
        let weak = Arc::downgrade(&gateway);
        let stop = gateway.stop.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                tokio::select! { _ = stop.cancelled() => break, _ = interval.tick() => {} }
                let Some(gateway) = weak.upgrade() else {
                    break;
                };
                gateway.expire_leases();
            }
        });
        Ok(gateway)
    }
    fn expire_leases(&self) {
        self.leases
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, lease| {
                let keep = lease.expires > std::time::Instant::now();
                if !keep {
                    lease.cancelled.cancel();
                }
                keep
            });
    }
    fn release_leases(&self, owner: &str) {
        self.leases
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|_, lease| {
                let keep = lease.owner != owner;
                if !keep {
                    lease.cancelled.cancel();
                }
                keep
            });
    }
    pub fn lease_session(
        self: &Arc<Self>,
        resolved: &mut super::plugins::resolve::Resolved,
    ) -> Arc<SessionLease> {
        self.expire_leases();
        let owner = uuid::Uuid::new_v4().to_string();
        for server in &mut resolved.servers {
            if let Some(metadata) = &server.gateway {
                let endpoint = self.issue_endpoint(&metadata.row_id, &metadata.resource, &owner);
                server.transport = brigadier_providers::McpTransport::Http {
                    url: endpoint.url.clone(),
                };
                server.env = vec![endpoint.secret_header.clone()];
                server.gateway = Some(endpoint);
            }
        }
        Arc::new(SessionLease {
            gateway: Arc::downgrade(self),
            owner,
        })
    }
    fn issue_endpoint(&self, row: &str, resource: &str, owner: &str) -> McpGateway {
        let secret = oauth::random();
        let generation = self
            .resource(resource)
            .grant_generation
            .load(Ordering::Acquire);
        self.leases
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                secret.clone(),
                Lease {
                    row: row.into(),
                    resource: resource.into(),
                    generation,
                    owner: owner.into(),
                    expires: std::time::Instant::now() + LEASE_TTL,
                    cancelled: CancellationToken::new(),
                },
            );
        let mut url = reqwest::Url::parse(&self.address).expect("loopback address");
        url.path_segments_mut()
            .expect("HTTP URL")
            .extend(["mcp", row]);
        McpGateway {
            url: url.into(),
            resource: resource.into(),
            row_id: row.into(),
            secret_header: (SECRET_HEADER.into(), secret),
        }
    }
    #[cfg(test)]
    pub(crate) fn test_endpoint(&self, row: &str, resource: &str) -> Option<McpGateway> {
        <Self as Gateway>::endpoint(self, row, resource)?;
        Some(self.issue_endpoint(row, resource, "fixture"))
    }
    fn resource(&self, resource: &str) -> Arc<Resource> {
        let mut resources = self.resources.lock().unwrap_or_else(|p| p.into_inner());
        resources
            .entry(resource.into())
            .or_insert_with(|| {
                let loaded = self.store.load(resource);
                let (grant, state) = match loaded {
                    Ok(Some(grant)) => (Some(grant), CatalogSignIn::SignedIn),
                    Ok(None) => (None, CatalogSignIn::SignedOut),
                    Err(_) => (
                        None,
                        CatalogSignIn::Failed {
                            message: "Cannot read gateway Keychain".into(),
                        },
                    ),
                };
                Arc::new(Resource {
                    granted: AtomicBool::new(grant.is_some()),
                    grant_generation: AtomicU64::new(0),
                    fallback: AtomicBool::new(false),
                    credentials: tokio::sync::Mutex::new(Credentials {
                        grant,
                        pending: None,
                        state,
                        generation: 0,
                        streams: CancellationToken::new(),
                    }),
                })
            })
            .clone()
    }
    pub async fn status(&self, resource: &str) -> CatalogSignIn {
        self.resource(resource)
            .credentials
            .lock()
            .await
            .state
            .clone()
    }
    /// Starts only after an explicit user action. Proxy requests never invoke this method.
    pub async fn start_sign_in(
        self: &Arc<Self>,
        resource: &str,
        open: Arc<dyn Fn(&str) + Send + Sync>,
    ) -> CatalogSignIn {
        let started = std::time::Instant::now();
        tracing::info!("MCP sign-in started");
        if oauth::url(resource).is_err() {
            tracing::info!(reason = "Invalid MCP resource URL", "MCP sign-in failed");
            return CatalogSignIn::Failed {
                message: "Invalid MCP resource URL".into(),
            };
        }
        let entry = self.resource(resource);
        let mut credentials = entry.credentials.lock().await;
        if matches!(
            credentials.state,
            CatalogSignIn::SigningIn { .. }
                | CatalogSignIn::SignedIn
                | CatalogSignIn::CliFallback { .. }
        ) {
            tracing::info!("MCP sign-in already active or signed in");
            return credentials.state.clone();
        }
        credentials.generation += 1;
        let generation = credentials.generation;
        credentials.state = CatalogSignIn::SigningIn {
            flow_id: oauth::random(),
        };
        let state = credentials.state.clone();
        drop(credentials);
        let gateway = self.clone();
        let resource = resource.to_owned();
        tokio::spawn(async move {
            let result = gateway.authorize(&resource, open).await;
            let mut credentials = entry.credentials.lock().await;
            if credentials.generation != generation {
                return;
            }
            credentials.state = match result {
                Ok(Some(grant)) => match gateway.store.save(&resource, &grant) {
                    Ok(()) => {
                        tracing::info!(
                            elapsed_ms = started.elapsed().as_millis() as u64,
                            "MCP sign-in token saved"
                        );
                        credentials.pending = None;
                        credentials.grant = Some(grant);
                        credentials.streams = CancellationToken::new();
                        entry.grant_generation.store(generation, Ordering::Release);
                        entry.granted.store(true, Ordering::Release);
                        entry.fallback.store(false, Ordering::Release);
                        CatalogSignIn::SignedIn
                    }
                    Err(_) => {
                        tracing::info!(
                            reason = "Cannot save gateway Keychain grant",
                            "MCP sign-in failed"
                        );
                        CatalogSignIn::Failed {
                            message: "Cannot save gateway Keychain grant".into(),
                        }
                    }
                },
                Ok(None) => {
                    tracing::info!("MCP sign-in requires CLI fallback");
                    entry.fallback.store(true, Ordering::Release);
                    CatalogSignIn::CliFallback {
                        steps: Vec::new(),
                        agents: vec![
                            brigadier_providers::ProviderKind::Claude,
                            brigadier_providers::ProviderKind::Codex,
                        ],
                    }
                }
                Err(message) => {
                    tracing::info!(reason = %message, "MCP sign-in failed");
                    CatalogSignIn::Failed { message }
                }
            };
        });
        state
    }
    async fn authorize(
        &self,
        resource: &str,
        open: Arc<dyn Fn(&str) + Send + Sync>,
    ) -> Result<Option<Grant>, String> {
        let started = std::time::Instant::now();
        let discovery = oauth::discover(&self.client, resource).await?;
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "MCP sign-in discovery completed"
        );
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| "Cannot listen for OAuth redirect")?;
        let redirect = format!(
            "http://127.0.0.1:{}/callback",
            listener
                .local_addr()
                .map_err(|_| "Cannot read OAuth redirect port")?
                .port()
        );
        let registration_started = std::time::Instant::now();
        let (id, secret) = match oauth::register(
            &self.client,
            &discovery.metadata,
            &redirect,
            self.client_document.as_deref(),
        )
        .await?
        {
            oauth::Registration::Client { id, secret } => (id, secret),
            oauth::Registration::Refused => return Ok(None),
        };
        tracing::info!(
            elapsed_ms = registration_started.elapsed().as_millis() as u64,
            "MCP sign-in registration completed"
        );
        let verifier = oauth::random();
        let state = oauth::random();
        let mut authorize = oauth::url(&discovery.metadata.authorization_endpoint)?;
        authorize.query_pairs_mut().extend_pairs([
            ("response_type", "code"),
            ("client_id", &id),
            ("redirect_uri", &redirect),
            ("code_challenge", &oauth::challenge(&verifier)),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("resource", resource),
            ("scope", &discovery.scopes),
        ]);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let callback = Arc::new(Callback {
            state,
            sender: Mutex::new(Some(sender)),
        });
        let router = Router::new()
            .route("/callback", axum::routing::get(callback_handler))
            .with_state(callback);
        let stop = CancellationToken::new();
        let ended = stop.clone();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(ended.cancelled_owned())
                .await;
        });
        open(authorize.as_str());
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            "MCP sign-in browser opened"
        );
        let code = tokio::time::timeout(std::time::Duration::from_secs(600), receiver).await;
        stop.cancel();
        let code = code
            .map_err(|_| "OAuth sign-in timed out")?
            .map_err(|_| "OAuth redirect closed")??;
        let grant = Grant {
            access_token: String::new(),
            refresh_token: None,
            expires_at: 0,
            client_id: id,
            client_secret: secret,
            token_endpoint: discovery.metadata.token_endpoint,
            revocation_endpoint: discovery.metadata.revocation_endpoint,
        };
        oauth::exchange(
            &self.client,
            resource,
            grant,
            vec![
                ("grant_type", "authorization_code".into()),
                ("code", code),
                ("redirect_uri", redirect),
                ("code_verifier", verifier),
            ],
        )
        .await
        .map(Some)
    }
    pub async fn sign_out(&self, resource: &str) -> io::Result<()> {
        let entry = self.resource(resource);
        let mut credentials = entry.credentials.lock().await;
        // Invalidate pending sign-ins and streams even if Keychain removal fails.
        credentials.generation += 1;
        credentials.streams.cancel();
        entry.granted.store(false, Ordering::Release);
        self.prompt_cache.invalidate(resource);
        entry.fallback.store(false, Ordering::Release);
        let grant = credentials
            .pending
            .take()
            .or_else(|| credentials.grant.take());
        credentials.grant = None;
        credentials.state = CatalogSignIn::SignedOut;
        let deleted = self.store.delete(resource);
        drop(credentials);
        if let Some(grant) = grant
            && let Some(endpoint) = &grant.revocation_endpoint
        {
            let token = grant.refresh_token.as_ref().unwrap_or(&grant.access_token);
            let mut form = vec![("token", token.clone()), ("client_id", grant.client_id)];
            if let Some(secret) = grant.client_secret {
                form.push(("client_secret", secret));
            }
            let _ = self
                .client
                .post(endpoint)
                .form(&form)
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await;
        }
        deleted
    }
    async fn reject_bearer_for_grant(
        &self,
        resource: &str,
        rejected: &str,
        generation: Option<u64>,
    ) {
        let entry = self.resource(resource);
        let mut credentials = entry.credentials.lock().await;
        if generation
            .is_some_and(|generation| entry.grant_generation.load(Ordering::Acquire) != generation)
        {
            return;
        }
        // A late 401 from an old request must not revoke a newly rotated grant.
        if credentials
            .grant
            .as_ref()
            .is_some_and(|grant| grant.access_token == rejected)
        {
            credentials.generation += 1;
            credentials.grant = None;
            credentials.state = CatalogSignIn::SignedOut;
            credentials.streams.cancel();
            entry.granted.store(false, Ordering::Release);
            self.prompt_cache.invalidate(resource);
            let _ = self.store.delete(resource);
        }
    }
    async fn bearer(&self, resource: &str) -> Result<(String, CancellationToken), StatusCode> {
        let entry = self.resource(resource);
        let mut credentials = entry.credentials.lock().await;
        if let Some(pending) = &credentials.pending {
            self.store
                .save(resource, pending)
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            credentials.grant = credentials.pending.take();
        }
        let Some(mut grant) = credentials.grant.clone() else {
            return Err(StatusCode::UNAUTHORIZED);
        };
        if grant.expires_at <= oauth::now().saturating_add(15) {
            let Some(refresh) = grant.refresh_token.clone() else {
                credentials.grant = None;
                credentials.state = CatalogSignIn::SignedOut;
                credentials.streams.cancel();
                entry.granted.store(false, Ordering::Release);
                self.prompt_cache.invalidate(resource);
                let _ = self.store.delete(resource);
                return Err(StatusCode::UNAUTHORIZED);
            };
            let result = oauth::exchange(
                &self.client,
                resource,
                grant,
                vec![
                    ("grant_type", "refresh_token".into()),
                    ("refresh_token", refresh),
                ],
            )
            .await;
            grant = match result {
                Ok(grant) => grant,
                Err(_) => {
                    credentials.grant = None;
                    credentials.state = CatalogSignIn::SignedOut;
                    credentials.streams.cancel();
                    entry.granted.store(false, Ordering::Release);
                    self.prompt_cache.invalidate(resource);
                    let _ = self.store.delete(resource);
                    return Err(StatusCode::UNAUTHORIZED);
                }
            };
            // Never publish a rotated access token before its corresponding refresh token
            // has been durably replaced. Hold the authority lock across refresh and commit.
            credentials.grant = None;
            credentials.pending = Some(grant.clone());
            self.store
                .save(resource, &grant)
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            credentials.grant = credentials.pending.take();
        }
        Ok((grant.access_token, credentials.streams.clone()))
    }
}
impl Gateway for McpGatewayService {
    fn endpoint(&self, row_id: &str, resource: &str) -> Option<McpGateway> {
        if oauth::url(resource).is_err() {
            return None;
        }
        let entry = self.resource(resource);
        if !entry.granted.load(Ordering::Acquire) || entry.fallback.load(Ordering::Acquire) {
            return None;
        }
        Some(McpGateway {
            url: resource.into(),
            resource: resource.into(),
            row_id: row_id.into(),
            secret_header: (SECRET_HEADER.into(), String::new()),
        })
    }
}

impl Gateway for Arc<McpGatewayService> {
    fn endpoint(&self, row_id: &str, resource: &str) -> Option<McpGateway> {
        <McpGatewayService as Gateway>::endpoint(self.as_ref(), row_id, resource)
    }
}

struct Callback {
    state: String,
    sender: Mutex<Option<tokio::sync::oneshot::Sender<Result<String, String>>>>,
}
async fn callback_handler(
    State(callback): State<Arc<Callback>>,
    axum::extract::Query(query): axum::extract::Query<HashMap<String, String>>,
) -> (StatusCode, &'static str) {
    if query.get("state") != Some(&callback.state) {
        tracing::info!(
            reason = "Invalid OAuth state",
            "MCP sign-in callback rejected"
        );
        return (StatusCode::BAD_REQUEST, "Invalid OAuth state");
    }
    let result = query
        .get("code")
        .filter(|code| !code.is_empty())
        .cloned()
        .ok_or_else(|| "OAuth authorization declined".into());
    if let Some(sender) = callback
        .sender
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
    {
        tracing::info!("MCP sign-in callback received");
        let _ = sender.send(result);
    }
    (StatusCode::OK, "You can return to Brigadier.")
}

fn hop_header(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    )
}
async fn proxy(
    State(gateway): State<std::sync::Weak<McpGatewayService>>,
    RoutePath(row): RoutePath<String>,
    request: Request<Body>,
) -> Result<Response<Body>, StatusCode> {
    let gateway = gateway.upgrade().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    if !matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::POST | axum::http::Method::DELETE
    ) {
        return Err(StatusCode::METHOD_NOT_ALLOWED);
    }
    // The secret is the capability. A browser origin cannot use the listener (including
    // same-machine web pages); do not offer CORS or forward a CLI's Authorization header.
    if request.headers().contains_key("origin") {
        return Err(StatusCode::FORBIDDEN);
    }
    let secret = request
        .headers()
        .get(SECRET_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let (resource, generation, lease_cancelled) = {
        let mut leases = gateway.leases.lock().unwrap_or_else(|p| p.into_inner());
        let lease = leases
            .get_mut(secret)
            .filter(|lease| lease.row == row && lease.expires > std::time::Instant::now())
            .ok_or(StatusCode::UNAUTHORIZED)?;
        lease.expires = std::time::Instant::now() + LEASE_TTL;
        (
            lease.resource.clone(),
            lease.generation,
            lease.cancelled.clone(),
        )
    };
    let entry = gateway.resource(&resource);
    if entry.grant_generation.load(Ordering::Acquire) != generation {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let (bearer, cancelled) = gateway.bearer(&resource).await?;
    // A sign-out/new sign-in can race the bearer lookup. Old session capabilities
    // must never acquire the replacement grant, even if the upstream reuses its token.
    if entry.grant_generation.load(Ordering::Acquire) != generation
        || cancelled.is_cancelled()
        || lease_cancelled.is_cancelled()
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let (parts, body) = request.into_parts();
    let mut upstream = gateway.client.request(parts.method, &resource);
    for (name, value) in &parts.headers {
        if !hop_header(name.as_str())
            && name != SECRET_HEADER
            && name != "authorization"
            && name != "cookie"
        {
            upstream = upstream.header(name, value);
        }
    }
    let response = upstream
        .bearer_auth(&bearer)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let cancelled = if response.status() == StatusCode::UNAUTHORIZED {
        gateway
            .reject_bearer_for_grant(&resource, &bearer, Some(generation))
            .await;
        // Forward the upstream 401, including its challenge/body, after revoking the grant.
        CancellationToken::new()
    } else {
        cancelled
    };
    let mut downstream = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if !hop_header(name.as_str()) && name != "set-cookie" && name != SECRET_HEADER {
            downstream = downstream.header(name, value);
        }
    }
    let stream = response.bytes_stream().take_until(async move {
        tokio::select! { _ = cancelled.cancelled() => {}, _ = lease_cancelled.cancelled() => {} }
    });
    downstream
        .body(Body::from_stream(stream))
        .map_err(|_| StatusCode::BAD_GATEWAY)
}
