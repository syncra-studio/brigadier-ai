use super::*;
use axum::{
    Json,
    extract::{Form, Query},
    response::{IntoResponse, Redirect},
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::atomic::AtomicUsize;
use store::MemoryStore;

/// Read-only catalog + public OAuth setup benchmark. Never opens a browser or exchanges a code.
#[tokio::test]
#[ignore = "requires explicit read-only catalog paths and a network connection"]
async fn sign_in_latency_measurement() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::INFO)
        .try_init();
    let home = std::env::var("BRIGADIER_SIGN_IN_BENCH_HOME").unwrap();
    let data = std::path::PathBuf::from(std::env::var("BRIGADIER_SIGN_IN_BENCH_DATA").unwrap());
    let project =
        std::path::PathBuf::from(std::env::var("BRIGADIER_SIGN_IN_BENCH_PROJECT").unwrap());
    let roots = brigadier_registry::Roots::from_vars([("HOME".into(), home)]).unwrap();
    let catalog_started = std::time::Instant::now();
    let rows = brigadier_registry::Catalog::read_rows_with_library(&roots, &[project], &data);
    let catalog_time = catalog_started.elapsed();
    assert!(
        rows.iter()
            .any(|row| row.endpoint.as_deref() == Some("https://mcp.atlassian.com/v2/mcp"))
    );
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    super::manager::remember_targets(&gateway, &rows, &data);
    let row = rows
        .iter()
        .find(|row| row.endpoint.as_deref() == Some("https://mcp.atlassian.com/v2/mcp"))
        .unwrap();
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let started = std::time::Instant::now();
    let (resource, _, _) = super::manager::cached_target(&gateway, &row.id)
        .unwrap()
        .unwrap();
    let lookup = started.elapsed();
    let network = std::time::Instant::now();
    gateway
        .start_sign_in(
            &resource,
            Arc::new(move |_| {
                let _ = sender.send(());
            }),
        )
        .await;
    tokio::time::timeout(std::time::Duration::from_secs(60), receiver.recv())
        .await
        .unwrap()
        .unwrap();
    eprintln!(
        "sign-in measurement: cold catalog={catalog_time:?}, cached lookup={lookup:?}, OAuth setup={:?}, click-to-browser={:?}",
        network.elapsed(),
        started.elapsed()
    );
}

#[tokio::test]
async fn sign_in_targets_use_the_page_snapshot_and_follow_catalog_refreshes() {
    use crate::manager::flow::{Flow, Options, Reply};
    let flow = Flow::start(
        "sign-in-target-cache",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("ok") })),
    )
    .await;
    let file = flow.manager.data_dir.join("library/servers/cached.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        r#"{"mcpServers":{"cached":{"type":"http","url":"https://cache.test/mcp"}}}"#,
    )
    .unwrap();
    let row = flow
        .manager
        .plugin_catalog()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.name == "cached")
        .unwrap();
    std::fs::remove_file(file).unwrap();
    // A status request uses the page's routing snapshot without reading the filesystem again.
    assert!(
        flow.manager
            .get_plugin_sign_in_status(&row.id)
            .await
            .is_ok()
    );
    // A fresh page read replaces that snapshot, including removals.
    flow.manager.plugin_catalog().unwrap();
    assert!(
        flow.manager
            .get_plugin_sign_in_status(&row.id)
            .await
            .is_err()
    );
    flow.stop().await;
}

#[derive(Default)]
struct FixtureState {
    origin: String,
    refused: AtomicBool,
    refreshes: AtomicUsize,
    revoked: AtomicBool,
    challenge: Mutex<String>,
    redirect: Mutex<String>,
    seen: Mutex<Vec<(String, String, Option<String>)>>,
    prompts: Mutex<Vec<Value>>,
    prompt_lists: AtomicUsize,
    prompt_blocked: AtomicBool,
    prompt_release: tokio::sync::Notify,
    prompt_sse: AtomicBool,
}
pub(crate) struct Fixture {
    state: Arc<FixtureState>,
    stop: CancellationToken,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Fixture {
    pub(crate) async fn new() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let state = Arc::new(FixtureState {
            origin: format!("http://{}", listener.local_addr().unwrap()),
            ..Default::default()
        });
        let router = Router::new()
            .route("/mcp", any(mcp))
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(resource_metadata),
            )
            .route(
                "/.well-known/oauth-authorization-server",
                get(auth_metadata),
            )
            .route("/register", post(register))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .route("/revoke", post(revoke))
            .with_state(state.clone());
        let stop = CancellationToken::new();
        let ended = stop.clone();
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(ended.cancelled_owned())
                .await
                .unwrap();
        });
        Self { state, stop }
    }
    pub(crate) fn resource(&self) -> String {
        format!("{}/mcp", self.state.origin)
    }
    pub(crate) fn set_prompts(&self, names: &[&str]) {
        *self.state.prompts.lock().unwrap() = names.iter().map(|name| json!({
            "name":name, "description":"A remote prompt", "arguments":[{"name":"topic", "required":true}]
        })).collect();
    }
    pub(crate) fn prompt_lists(&self) -> usize {
        self.state.prompt_lists.load(Ordering::SeqCst)
    }
    async fn wait_prompt_lists(&self, count: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while self.prompt_lists() < count {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    fn browser(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        Arc::new(|url| {
            let url = url.to_owned();
            tokio::spawn(async move {
                let response = reqwest::get(url).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert_eq!(
                    response.text().await.unwrap(),
                    "You can close this tab and return to Brigadier."
                );
            });
        })
    }
    pub(crate) async fn signed_in(&self, gateway: &Arc<McpGatewayService>) {
        let state = gateway
            .start_sign_in(&self.resource(), self.browser())
            .await;
        assert!(matches!(state, CatalogSignIn::SigningIn { .. }));
        for _ in 0..200 {
            match gateway.status(&self.resource()).await {
                CatalogSignIn::SignedIn => return,
                CatalogSignIn::SigningIn { .. } => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await
                }
                other => panic!("Sign-in failed: {other:?}"),
            }
        }
        panic!("sign-in fixture timed out");
    }
}
async fn resource_metadata(State(state): State<Arc<FixtureState>>) -> Json<Value> {
    Json(
        json!({"resource":format!("{}/mcp", state.origin), "authorization_servers":[state.origin], "scopes_supported":["tools"]}),
    )
}
async fn auth_metadata(State(state): State<Arc<FixtureState>>) -> Json<Value> {
    Json(
        json!({"issuer":state.origin, "authorization_endpoint":format!("{}/authorize", state.origin), "token_endpoint":format!("{}/token", state.origin), "registration_endpoint":format!("{}/register", state.origin), "revocation_endpoint":format!("{}/revoke", state.origin), "code_challenge_methods_supported":["S256"]}),
    )
}
async fn register(
    State(state): State<Arc<FixtureState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    if state.refused.load(Ordering::SeqCst) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"unauthorized_client"})),
        );
    }
    assert_eq!(body["token_endpoint_auth_method"], "none");
    assert_eq!(body["client_name"], "Brigadier");
    *state.redirect.lock().unwrap() = body["redirect_uris"][0].as_str().unwrap().into();
    (
        StatusCode::CREATED,
        Json(json!({"client_id":"brigadier-fixture-client"})),
    )
}
async fn authorize(
    State(state): State<Arc<FixtureState>>,
    Query(query): Query<HashMap<String, String>>,
) -> Redirect {
    assert_eq!(query["client_id"], "brigadier-fixture-client");
    assert_eq!(query["resource"], format!("{}/mcp", state.origin));
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(query["redirect_uri"], *state.redirect.lock().unwrap());
    *state.challenge.lock().unwrap() = query["code_challenge"].clone();
    let mut callback = reqwest::Url::parse(&query["redirect_uri"]).unwrap();
    // Invalid state must leave the valid callback alive.
    callback
        .query_pairs_mut()
        .extend_pairs([("code", "wrong"), ("state", "wrong")]);
    assert_eq!(
        reqwest::get(callback.clone()).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    callback.set_query(None);
    callback
        .query_pairs_mut()
        .extend_pairs([("code", "fixture-code"), ("state", &query["state"])]);
    Redirect::to(callback.as_str())
}
async fn token(
    State(state): State<Arc<FixtureState>>,
    Form(form): Form<HashMap<String, String>>,
) -> Json<Value> {
    assert_eq!(form["client_id"], "brigadier-fixture-client");
    assert_eq!(form["resource"], format!("{}/mcp", state.origin));
    let rotation = if form["grant_type"] == "authorization_code" {
        assert_eq!(form["code"], "fixture-code");
        assert_eq!(form["redirect_uri"], *state.redirect.lock().unwrap());
        assert_eq!(
            oauth::challenge(&form["code_verifier"]),
            *state.challenge.lock().unwrap()
        );
        0
    } else {
        let previous = state.refreshes.fetch_add(1, Ordering::SeqCst);
        assert_eq!(form["refresh_token"], format!("refresh-{previous}"));
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        previous + 1
    };
    Json(
        json!({"access_token":format!("access-{rotation}"), "refresh_token":format!("refresh-{rotation}"), "expires_in":3600, "token_type":"Bearer"}),
    )
}
async fn revoke(
    State(state): State<Arc<FixtureState>>,
    Form(form): Form<HashMap<String, String>>,
) -> StatusCode {
    assert!(form["token"].starts_with("refresh-"));
    state.revoked.store(true, Ordering::SeqCst);
    StatusCode::OK
}
async fn mcp(State(state): State<Arc<FixtureState>>, request: Request<Body>) -> Response<Body> {
    assert!(!request.headers().contains_key(SECRET_HEADER));
    let token = format!("Bearer access-{}", state.refreshes.load(Ordering::SeqCst));
    if request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(token.as_str())
        || state.revoked.load(Ordering::SeqCst)
    {
        return Response::builder()
            .status(401)
            .header(
                "www-authenticate",
                format!(
                    "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"",
                    state.origin
                ),
            )
            .body(Body::empty())
            .unwrap();
    }
    let method = request.method().clone();
    let version = request
        .headers()
        .get("mcp-protocol-version")
        .map(|value| value.to_str().unwrap().to_owned());
    let session = request
        .headers()
        .get("mcp-session-id")
        .map(|value| value.to_str().unwrap().to_owned());
    let body = axum::body::to_bytes(request.into_body(), 1024 * 1024)
        .await
        .unwrap();
    state.seen.lock().unwrap().push((
        method.to_string(),
        String::from_utf8(body.to_vec()).unwrap(),
        session.clone(),
    ));
    if method == axum::http::Method::POST
        && let Ok(message) = serde_json::from_slice::<Value>(&body)
    {
        if message.get("method").and_then(Value::as_str) == Some("initialize") {
            assert_eq!(
                message
                    .pointer("/params/clientInfo/name")
                    .and_then(Value::as_str),
                Some("Brigadier")
            );
            return Response::builder().header("mcp-session-id", "prompt-session").header("content-type", "application/json").body(Body::from(json!({
                "jsonrpc":"2.0", "id":message["id"], "result":{"protocolVersion":"2025-11-25", "capabilities":{"prompts":{}}}
            }).to_string())).unwrap();
        }
        if message.get("method").and_then(Value::as_str) == Some("prompts/list") {
            assert_eq!(session.as_deref(), Some("prompt-session"));
            assert_eq!(version.as_deref(), Some("2025-11-25"));
            state.prompt_lists.fetch_add(1, Ordering::SeqCst);
            let release = state.prompt_release.notified();
            if state.prompt_blocked.load(Ordering::SeqCst) {
                release.await;
            }
            let prompts = state.prompts.lock().unwrap().clone();
            let mut result = if message.pointer("/params/cursor").is_some() {
                json!({"prompts":prompts.into_iter().skip(1).collect::<Vec<_>>()})
            } else {
                let next = prompts.len() > 1;
                let mut result = json!({"prompts":prompts.into_iter().take(1).collect::<Vec<_>>()});
                if next {
                    result["nextCursor"] = json!("page-two");
                }
                result
            };
            // Keep the prompt names remote-controlled so cache refresh tests detect changes.
            let response =
                json!({"jsonrpc":"2.0", "id":message["id"], "result":result.take()}).to_string();
            if state.prompt_sse.load(Ordering::SeqCst) {
                let middle = response.len() / 2;
                let chunks = vec![
                    Ok::<_, io::Error>(": keepalive\r\n\r\nevent: message\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/prompts/list_changed\"}\r\n\r\ndata: ".to_owned()),
                    Ok(response[..middle].to_owned()), Ok(format!("{}\r\n\r\n", &response[middle..])),
                ];
                return Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(Body::from_stream(futures_util::stream::iter(chunks)))
                    .unwrap();
            }
            return Response::builder()
                .header("content-type", "application/json")
                .body(Body::from(response))
                .unwrap();
        }
    }
    match method {
        axum::http::Method::GET => {
            let first = futures_util::stream::once(async {
                Ok::<_, io::Error>(
                    "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
                )
            });
            let stream = first.chain(futures_util::stream::pending());
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }
        axum::http::Method::DELETE => Response::builder().status(204).body(Body::empty()).unwrap(),
        _ if !body.is_empty()
            && serde_json::from_slice::<Value>(&body)
                .unwrap()
                .get("id")
                .is_none() =>
        {
            Response::builder().status(202).body(Body::empty()).unwrap()
        }
        _ => Response::builder()
            .header("mcp-session-id", "session42")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"jsonrpc":"2.0", "id":1, "result":{"tools":[{"name":"fixture-tool"}]}})
                    .to_string(),
            ))
            .unwrap(),
    }
}
fn request(
    client: &reqwest::Client,
    endpoint: &McpGateway,
    method: reqwest::Method,
) -> reqwest::RequestBuilder {
    client
        .request(method, &endpoint.url)
        .header(&endpoint.secret_header.0, &endpoint.secret_header.1)
}

#[tokio::test]
async fn mcp_gateway_dcr_pkce_streamable_http_and_rotation_across_two_sessions() {
    let fixture = Fixture::new().await;
    let store = Arc::new(MemoryStore::default());
    let gateway = McpGatewayService::with_store(store.clone(), None).unwrap();
    assert!(gateway.test_endpoint("row", &fixture.resource()).is_none());
    fixture.signed_in(&gateway).await;
    let first = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    let second = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    assert_ne!(first.secret_header, second.secret_header);
    assert!(!format!("{first:?}").contains(&first.secret_header.1));
    let client = reqwest::Client::new();
    let response = request(&client, &first, reqwest::Method::POST)
        .header("authorization", "Bearer cli-token-must-never-pass-through")
        .json(&json!({"jsonrpc":"2.0", "id":1, "method":"tools/list"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["mcp-session-id"], "session42");
    assert_eq!(
        response.json::<Value>().await.unwrap()["result"]["tools"][0]["name"],
        "fixture-tool"
    );
    let mut events = request(&client, &second, reqwest::Method::GET)
        .header("mcp-session-id", "session42")
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), events.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        std::str::from_utf8(&event)
            .unwrap()
            .contains("notifications/tools/list_changed")
    );
    for method in ["notifications/initialized", "notifications/cancelled"] {
        assert_eq!(
            request(&client, &first, reqwest::Method::POST)
                .header("mcp-session-id", "session42")
                .json(&json!({"jsonrpc":"2.0", "method":method, "params":{"requestId":2}}))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(
        request(&client, &second, reqwest::Method::DELETE)
            .header("mcp-session-id", "session42")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    {
        let entry = gateway.resource(&fixture.resource());
        let mut credentials = entry.credentials.lock().await;
        credentials.grant.as_mut().unwrap().expires_at = 0;
        store
            .save(&fixture.resource(), credentials.grant.as_ref().unwrap())
            .unwrap();
    }
    let call = |endpoint: McpGateway| {
        let client = client.clone();
        async move {
            request(&client, &endpoint, reqwest::Method::POST)
                .json(&json!({"jsonrpc":"2.0", "id":1, "method":"tools/list"}))
                .send()
                .await
                .unwrap()
                .status()
        }
    };
    let (a, b) = tokio::join!(call(first.clone()), call(second.clone()));
    assert_eq!((a, b), (StatusCode::OK, StatusCode::OK));
    assert_eq!(fixture.state.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .load(&fixture.resource())
            .unwrap()
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("refresh-1")
    );
    gateway.sign_out(&fixture.resource()).await.unwrap();
    assert!(fixture.state.revoked.load(Ordering::SeqCst));
    assert!(store.load(&fixture.resource()).unwrap().is_none());
    for endpoint in [first, second] {
        assert_eq!(
            request(&client, &endpoint, reqwest::Method::GET)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(2), events.chunk())
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    let seen = fixture.state.seen.lock().unwrap();
    assert!(
        seen.iter()
            .any(|(method, _, session)| method == "GET" && session.as_deref() == Some("session42"))
    );
    assert!(
        seen.iter()
            .any(|(_, body, _)| body.contains("notifications/cancelled"))
    );
}

#[tokio::test]
async fn mcp_gateway_rejects_missing_wrong_secret_cross_row_and_browser_origin() {
    let fixture = Fixture::new().await;
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    fixture.signed_in(&gateway).await;
    let endpoint = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    let client = reqwest::Client::new();
    assert_eq!(
        client.get(&endpoint.url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(&endpoint.url)
            .header(SECRET_HEADER, "wrong")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&client, &endpoint, reqwest::Method::GET)
            .header("origin", "https://evil.test")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let mut other = endpoint;
    other.url = other.url.replace("/row", "/other");
    assert_eq!(
        request(&client, &other, reqwest::Method::GET)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn mcp_gateway_refused_dcr_is_cli_fallback_without_opening_browser() {
    let fixture = Fixture::new().await;
    fixture.state.refused.store(true, Ordering::SeqCst);
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    gateway
        .start_sign_in(
            &fixture.resource(),
            Arc::new(|_| panic!("Must not open browser for refused registration")),
        )
        .await;
    for _ in 0..100 {
        if matches!(
            gateway.status(&fixture.resource()).await,
            CatalogSignIn::CliFallback { .. }
        ) {
            assert!(gateway.test_endpoint("row", &fixture.resource()).is_none());
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("Fallback was not reported");
}

#[test]
fn mcp_gateway_keychain_instance_separation_and_https_validation() {
    assert_ne!(
        store::KeychainStore::new(Path::new("/tmp/brigadier-dev-instance")).service_name(),
        store::KeychainStore::new(Path::new("/tmp/brigadier-installed-instance")).service_name()
    );
    assert!(oauth::url("http://remote.test/mcp").is_err());
    assert!(oauth::url("https://user:password@remote.test/mcp").is_err());
    assert!(oauth::url("https://remote.test/mcp#fragment").is_err());
}

#[tokio::test]
async fn mcp_gateway_upstream_revocation_invalidates_every_session_and_returns_401() {
    let fixture = Fixture::new().await;
    let store = Arc::new(MemoryStore::default());
    let gateway = McpGatewayService::with_store(store.clone(), None).unwrap();
    fixture.signed_in(&gateway).await;
    let first = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    let second = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    fixture.state.revoked.store(true, Ordering::SeqCst);
    let client = reqwest::Client::new();
    let response = request(&client, &first, reqwest::Method::GET)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(response.headers().contains_key("www-authenticate"));
    assert_eq!(
        gateway.status(&fixture.resource()).await,
        CatalogSignIn::SignedOut
    );
    assert!(store.load(&fixture.resource()).unwrap().is_none());
    assert_eq!(
        request(&client, &second, reqwest::Method::GET)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn mcp_gateway_sign_out_during_rotation_cannot_resurrect_the_grant() {
    let fixture = Fixture::new().await;
    let store = Arc::new(MemoryStore::default());
    let gateway = McpGatewayService::with_store(store.clone(), None).unwrap();
    fixture.signed_in(&gateway).await;
    let endpoint = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    {
        let entry = gateway.resource(&fixture.resource());
        entry
            .credentials
            .lock()
            .await
            .grant
            .as_mut()
            .unwrap()
            .expires_at = 0;
    }
    let client = reqwest::Client::new();
    let sending = tokio::spawn(
        request(&client, &endpoint, reqwest::Method::POST)
            .json(&json!({"id":1,"method":"tools/list"}))
            .send(),
    );
    for _ in 0..100 {
        if fixture.state.refreshes.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(fixture.state.refreshes.load(Ordering::SeqCst), 1);
    gateway.sign_out(&fixture.resource()).await.unwrap();
    let _ = sending.await.unwrap().unwrap();
    assert!(store.load(&fixture.resource()).unwrap().is_none());
    assert_eq!(
        gateway.status(&fixture.resource()).await,
        CatalogSignIn::SignedOut
    );
    assert_eq!(
        request(&client, &endpoint, reqwest::Method::GET)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn mcp_gateway_client_metadata_document_is_our_client_id_when_supported() {
    let fixture = Fixture::new().await;
    fixture.state.refused.store(true, Ordering::SeqCst);
    let metadata = oauth::Metadata {
        issuer: fixture.state.origin.clone(),
        authorization_endpoint: format!("{}/authorize", fixture.state.origin),
        token_endpoint: format!("{}/token", fixture.state.origin),
        registration_endpoint: Some(format!("{}/register", fixture.state.origin)),
        revocation_endpoint: None,
        code_challenge_methods_supported: vec!["S256".into()],
        client_id_metadata_document_supported: true,
    };
    let client = reqwest::Client::new();
    let registration = oauth::register(
        &client,
        &metadata,
        "http://127.0.0.1:1234/callback",
        Some("https://brigadier.example/oauth/client.json"),
    )
    .await
    .unwrap();
    assert!(
        matches!(registration, oauth::Registration::Client { id, secret: None } if id == "https://brigadier.example/oauth/client.json")
    );
    assert!(
        oauth::register(
            &client,
            &metadata,
            "http://127.0.0.1:1234/callback",
            Some("http://127.0.0.1/client.json")
        )
        .await
        .is_err()
    );
}

fn prompt_server(
    gateway: &McpGatewayService,
    row: &str,
    resource: &str,
) -> brigadier_providers::McpServer {
    let endpoint = gateway.test_endpoint(row, resource).unwrap();
    brigadier_providers::McpServer {
        gateway: Some(endpoint.clone()),
        name: "db".into(),
        transport: brigadier_providers::McpTransport::Http { url: endpoint.url },
        env: vec![endpoint.secret_header],
        tool_timeout_secs: None,
        trusted: false,
        always_load: false,
    }
}

#[tokio::test]
async fn mcp_gateway_prompt_cache_handles_sse_pagination_and_terminates_its_own_session() {
    let fixture = Fixture::new().await;
    fixture.set_prompts(&["review", "summarize"]);
    fixture.state.prompt_sse.store(true, Ordering::SeqCst);
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    fixture.signed_in(&gateway).await;
    let server = prompt_server(&gateway, "row", &fixture.resource());
    assert!(
        gateway
            .cached_prompts(std::slice::from_ref(&server))
            .is_empty()
    );
    gateway.wait_for_prompt_cache().await;
    let prompts = gateway.cached_prompts(std::slice::from_ref(&server));
    assert_eq!(
        prompts
            .iter()
            .map(|prompt| prompt.name.as_str())
            .collect::<Vec<_>>(),
        ["review", "summarize"]
    );
    assert_eq!(fixture.prompt_lists(), 2);
    let seen = fixture.state.seen.lock().unwrap();
    assert!(
        seen.iter().any(|(method, _, session)| method == "DELETE"
            && session.as_deref() == Some("prompt-session"))
    );
    assert!(seen.iter().any(
        |(_, body, session)| body.contains("notifications/initialized")
            && session.as_deref() == Some("prompt-session")
    ));
}

#[tokio::test]
async fn mcp_gateway_prompt_cache_returns_stale_entries_while_one_background_refresh_is_pending() {
    let fixture = Fixture::new().await;
    fixture.set_prompts(&["before"]);
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    fixture.signed_in(&gateway).await;
    let servers = [prompt_server(&gateway, "row", &fixture.resource())];
    assert!(gateway.cached_prompts(&servers).is_empty());
    gateway.wait_for_prompt_cache().await;
    assert_eq!(gateway.cached_prompts(&servers)[0].name, "before");
    fixture.set_prompts(&["after"]);
    fixture.state.prompt_blocked.store(true, Ordering::SeqCst);
    gateway.expire_prompt_cache();
    assert_eq!(gateway.cached_prompts(&servers)[0].name, "before");
    fixture.wait_prompt_lists(2).await;
    // The fixture is blocked: repeated menu reads must still return, and must deduplicate
    // the refresh instead of spawning more sessions for this row/grant.
    assert_eq!(gateway.cached_prompts(&servers)[0].name, "before");
    assert_eq!(fixture.prompt_lists(), 2);
    fixture.state.prompt_blocked.store(false, Ordering::SeqCst);
    fixture.state.prompt_release.notify_one();
    gateway.wait_for_prompt_cache().await;
    assert_eq!(gateway.cached_prompts(&servers)[0].name, "after");
    assert_eq!(fixture.prompt_lists(), 2);
}

#[tokio::test]
async fn mcp_gateway_prompt_cache_is_discarded_on_sign_out_and_new_grants_start_empty() {
    let fixture = Fixture::new().await;
    fixture.set_prompts(&["old-grant"]);
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    fixture.signed_in(&gateway).await;
    let servers = [prompt_server(&gateway, "row", &fixture.resource())];
    assert!(gateway.cached_prompts(&servers).is_empty());
    gateway.wait_for_prompt_cache().await;
    assert_eq!(gateway.cached_prompts(&servers)[0].name, "old-grant");
    gateway.sign_out(&fixture.resource()).await.unwrap();
    assert!(gateway.cached_prompts(&servers).is_empty());
    fixture.state.revoked.store(false, Ordering::SeqCst);
    fixture.set_prompts(&["new-grant"]);
    fixture.signed_in(&gateway).await;
    assert!(gateway.cached_prompts(&servers).is_empty());
    gateway.wait_for_prompt_cache().await;
    assert_eq!(
        fixture.prompt_lists(),
        1,
        "the revoked capability stays unusable"
    );
    let servers = [prompt_server(&gateway, "row", &fixture.resource())];
    assert!(gateway.cached_prompts(&servers).is_empty());
    gateway.wait_for_prompt_cache().await;
    assert_eq!(gateway.cached_prompts(&servers)[0].name, "new-grant");
    assert_eq!(fixture.prompt_lists(), 2);
}

#[tokio::test]
async fn mcp_gateway_prompt_cache_is_per_row_and_shared_by_local_session_capabilities() {
    let fixture = Fixture::new().await;
    fixture.set_prompts(&["review"]);
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    fixture.signed_in(&gateway).await;
    let first = [prompt_server(&gateway, "row-one", &fixture.resource())];
    let second = [prompt_server(&gateway, "row-one", &fixture.resource())];
    let other = [prompt_server(&gateway, "row-two", &fixture.resource())];
    assert!(gateway.cached_prompts(&first).is_empty());
    gateway.wait_for_prompt_cache().await;
    assert_eq!(gateway.cached_prompts(&second)[0].name, "review");
    assert_eq!(fixture.prompt_lists(), 1);
    assert!(gateway.cached_prompts(&other).is_empty());
    gateway.wait_for_prompt_cache().await;
    assert_eq!(gateway.cached_prompts(&other)[0].name, "review");
    assert_eq!(fixture.prompt_lists(), 2);
}

#[tokio::test]
async fn mcp_gateway_old_capabilities_stay_revoked_after_signing_in_again() {
    let fixture = Fixture::new().await;
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    fixture.signed_in(&gateway).await;
    let old = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    gateway.sign_out(&fixture.resource()).await.unwrap();
    fixture.state.revoked.store(false, Ordering::SeqCst);
    fixture.signed_in(&gateway).await;
    let current = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    let client = reqwest::Client::new();
    for (endpoint, expected) in [(old, StatusCode::UNAUTHORIZED), (current, StatusCode::OK)] {
        assert_eq!(
            request(&client, &endpoint, reqwest::Method::POST)
                .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
                .send()
                .await
                .unwrap()
                .status(),
            expected
        );
    }
}

#[tokio::test]
async fn rotated_grant_waits_for_durable_retry_without_reusing_the_refresh_token() {
    struct Flaky {
        inner: MemoryStore,
        fail: AtomicBool,
    }
    impl TokenStore for Flaky {
        fn load(&self, r: &str) -> io::Result<Option<Grant>> {
            self.inner.load(r)
        }
        fn save(&self, r: &str, g: &Grant) -> io::Result<()> {
            if self.fail.swap(false, Ordering::SeqCst) {
                return Err(io::Error::other("fixture save failure"));
            }
            self.inner.save(r, g)
        }
        fn delete(&self, r: &str) -> io::Result<()> {
            self.inner.delete(r)
        }
    }
    let fixture = Fixture::new().await;
    let store = Arc::new(Flaky {
        inner: MemoryStore::default(),
        fail: AtomicBool::new(false),
    });
    let gateway = McpGatewayService::with_store(store.clone(), None).unwrap();
    fixture.signed_in(&gateway).await;
    let endpoint = gateway.test_endpoint("row", &fixture.resource()).unwrap();
    gateway
        .resource(&fixture.resource())
        .credentials
        .lock()
        .await
        .grant
        .as_mut()
        .unwrap()
        .expires_at = 0;
    store.fail.store(true, Ordering::SeqCst);
    let client = reqwest::Client::new();
    for expected in [StatusCode::SERVICE_UNAVAILABLE, StatusCode::OK] {
        assert_eq!(
            request(&client, &endpoint, reqwest::Method::POST)
                .json(&json!({"id":1,"method":"tools/list"}))
                .send()
                .await
                .unwrap()
                .status(),
            expected
        );
    }
    assert_eq!(fixture.state.refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        store
            .load(&fixture.resource())
            .unwrap()
            .unwrap()
            .refresh_token
            .as_deref(),
        Some("refresh-1")
    );
}

#[tokio::test]
async fn metadata_reads_mint_no_leases_and_session_release_and_ttl_revoke_capabilities() {
    let fixture = Fixture::new().await;
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    fixture.signed_in(&gateway).await;
    for _ in 0..100 {
        assert!(Gateway::endpoint(gateway.as_ref(), "row", &fixture.resource()).is_some());
    }
    assert!(gateway.leases.lock().unwrap().is_empty());
    let mut server = prompt_server(&gateway, "row", &fixture.resource());
    gateway.release_leases("fixture");
    server.gateway = Gateway::endpoint(gateway.as_ref(), "row", &fixture.resource());
    let mut resolved = super::super::plugins::resolve::Resolved {
        servers: vec![server],
        ..Default::default()
    };
    let lease = gateway.lease_session(&mut resolved);
    let endpoint = resolved.servers[0].gateway.clone().unwrap();
    assert_eq!(gateway.leases.lock().unwrap().len(), 1);
    lease.release();
    assert!(gateway.leases.lock().unwrap().is_empty());
    assert_eq!(
        request(&reqwest::Client::new(), &endpoint, reqwest::Method::GET)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let _lease = gateway.lease_session(&mut resolved);
    for lease in gateway.leases.lock().unwrap().values_mut() {
        lease.expires = std::time::Instant::now() - std::time::Duration::from_secs(1);
    }
    gateway.expire_leases();
    assert!(gateway.leases.lock().unwrap().is_empty());
}

#[tokio::test]
async fn library_only_refusing_server_requires_native_setup_without_running_native_login() {
    use crate::manager::flow::{Flow, Options, Reply};
    let fixture = Fixture::new().await;
    fixture.state.refused.store(true, Ordering::SeqCst);
    let flow = Flow::start(
        "library-refusing-status",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("ok") })),
    )
    .await;
    let file = flow.manager.data_dir.join("library/servers/refusing.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        json!({"mcpServers":{"refusing":{"type":"http","url":fixture.resource()}}}).to_string(),
    )
    .unwrap();
    let row = flow
        .manager
        .plugin_catalog()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.name == "refusing")
        .unwrap();
    let open: Arc<dyn Fn(&str) + Send + Sync> =
        Arc::new(|_| panic!("refusing client must not open a browser"));
    assert!(flow.manager.sign_in_browser.set(open.clone()).is_ok());
    flow.manager
        .mcp_gateway()
        .start_sign_in(&fixture.resource(), open)
        .await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while matches!(
            flow.manager.mcp_gateway().status(&fixture.resource()).await,
            CatalogSignIn::SigningIn { .. }
        ) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for state in [
        flow.manager
            .get_plugin_sign_in_status(&row.id)
            .await
            .unwrap(),
        flow.manager.start_plugin_sign_in(&row.id).await.unwrap(),
    ] {
        assert!(
            matches!(state, CatalogSignIn::CliSetupRequired { agents } if agents == brigadier_providers::ProviderKind::ALL)
        );
    }
    flow.stop().await;
}
