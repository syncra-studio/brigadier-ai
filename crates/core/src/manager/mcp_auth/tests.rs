//! Sign-ins read from fake stores (a Keychain in memory and the CLIs' files in a home of the
//! test's own), served through the proxy to a fake MCP server, and renewed and signed in
//! against a fake OAuth server, both on 127.0.0.1. No test touches the user's Keychain or files.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use brigadier_registry::{Agent, Catalog, Session, SignIn, claude_key, codex_keys};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, LOCATION, WWW_AUTHENTICATE};
use hyper::{Request, Response, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::stores::{Credential, Found, now_ms};
use super::*;

/// What the fake servers take and saw.
#[derive(Default)]
struct State {
    /// The access tokens the MCP server takes.
    valid: HashSet<String>,
    /// The refresh tokens the token endpoint takes: each once (rotation).
    refresh: HashSet<String>,
    /// The client authentication the token endpoint requires (`Basic …`).
    basic: Option<String>,
    /// Codes handed out, with their PKCE challenge.
    codes: HashMap<String, String>,
    /// Every token handed out.
    issued: Vec<String>,
    /// The `Authorization` of each request to the MCP server.
    seen: Vec<String>,
    refreshes: u32,
}

/// A fake MCP server (`/mcp`) that is its own OAuth authorization server.
struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
}

fn reply(status: StatusCode, value: &Value) -> Response<Full<Bytes>> {
    let mut response = Response::new(Full::new(Bytes::from(value.to_string())));
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, "application/json".parse().unwrap());
    response
}

fn form(text: &[u8]) -> HashMap<String, String> {
    url::form_urlencoded::parse(text).into_owned().collect()
}

fn issue(state: &mut State) -> Value {
    let access = format!("access-{}", uuid::Uuid::new_v4().simple());
    let refresh = format!("refresh-{}", uuid::Uuid::new_v4().simple());
    state.valid.insert(access.clone());
    state.refresh.insert(refresh.clone());
    state.issued.extend([access.clone(), refresh.clone()]);
    json!({"access_token": access, "refresh_token": refresh, "expires_in": 3600,
           "token_type": "Bearer"})
}

async fn fake(
    base: &str,
    state: &Mutex<State>,
    request: Request<Incoming>,
) -> Response<Full<Bytes>> {
    let (parts, body) = request.into_parts();
    let body = body.collect().await.unwrap().to_bytes();
    let header = |name| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let query = form(parts.uri.query().unwrap_or_default().as_bytes());
    let mut state = state.lock().unwrap();
    match (parts.method.as_str(), parts.uri.path()) {
        ("POST", "/mcp") => {
            let auth = header(AUTHORIZATION).unwrap_or_default();
            state.seen.push(auth.clone());
            if auth
                .strip_prefix("Bearer ")
                .is_some_and(|token| state.valid.contains(token))
            {
                return reply(
                    StatusCode::OK,
                    &json!({"jsonrpc": "2.0", "id": 1, "result": {}}),
                );
            }
            let mut refused = reply(StatusCode::UNAUTHORIZED, &json!({"error": "invalid_token"}));
            refused.headers_mut().insert(
                WWW_AUTHENTICATE,
                format!(
                    r#"Bearer resource_metadata="{base}/.well-known/oauth-protected-resource/mcp""#
                )
                .parse()
                .unwrap(),
            );
            refused
        }
        ("GET", "/.well-known/oauth-protected-resource/mcp") => reply(
            StatusCode::OK,
            &json!({"resource": format!("{base}/mcp"), "authorization_servers": [base],
                    "scopes_supported": ["read"]}),
        ),
        ("GET", "/.well-known/oauth-authorization-server") => reply(
            StatusCode::OK,
            &json!({"issuer": base, "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "registration_endpoint": format!("{base}/register"),
                    "code_challenge_methods_supported": ["S256"]}),
        ),
        ("POST", "/register") => reply(StatusCode::CREATED, &json!({"client_id": "registered"})),
        ("GET", "/authorize") => {
            assert_eq!(query["code_challenge_method"], "S256");
            assert_eq!(query["client_id"], "registered");
            assert_eq!(query["resource"], format!("{base}/mcp"));
            let code = uuid::Uuid::new_v4().simple().to_string();
            state
                .codes
                .insert(code.clone(), query["code_challenge"].clone());
            let mut back = Response::new(Full::new(Bytes::new()));
            *back.status_mut() = StatusCode::FOUND;
            back.headers_mut().insert(
                LOCATION,
                format!(
                    "{}?code={code}&state={}",
                    query["redirect_uri"], query["state"]
                )
                .parse()
                .unwrap(),
            );
            back
        }
        ("POST", "/token") => {
            if let Some(basic) = &state.basic
                && header(AUTHORIZATION).as_ref() != Some(basic)
            {
                return reply(
                    StatusCode::UNAUTHORIZED,
                    &json!({"error": "invalid_client"}),
                );
            }
            let form = form(&body);
            match form["grant_type"].as_str() {
                "refresh_token" if state.refresh.remove(&form["refresh_token"]) => {
                    state.refreshes += 1;
                    reply(StatusCode::OK, &issue(&mut state))
                }
                "authorization_code"
                    if state.codes.remove(&form["code"]).is_some_and(|challenge| {
                        challenge
                            == base64::engine::general_purpose::URL_SAFE_NO_PAD
                                .encode(Sha256::digest(form["code_verifier"].as_bytes()))
                    }) =>
                {
                    reply(StatusCode::OK, &issue(&mut state))
                }
                _ => reply(StatusCode::BAD_REQUEST, &json!({"error": "invalid_grant"})),
            }
        }
        _ => reply(StatusCode::NOT_FOUND, &json!({})),
    }
}

impl Fake {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let (serving, shared) = (base.clone(), state.clone());
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let (base, state) = (serving.clone(), shared.clone());
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |request| {
                        let (base, state) = (base.clone(), state.clone());
                        async move { Ok::<_, Infallible>(fake(&base, &state, request).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        Self { base, state }
    }

    fn url(&self) -> String {
        format!("{}/mcp", self.base)
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// A token pair it takes, as an agent's sign-in left it.
    fn signed_in(&self) -> (String, String) {
        let answer = issue(&mut self.state());
        (
            answer["access_token"].as_str().unwrap().to_owned(),
            answer["refresh_token"].as_str().unwrap().to_owned(),
        )
    }

    /// Every access token stops working, as one does when it expires.
    fn expire(&self) {
        self.state().valid.clear();
    }

    /// Whether the token endpoint takes `refresh` now, as the owning CLI would send it.
    async fn takes_refresh(&self, refresh: &str) -> bool {
        let basic = self.state().basic.clone();
        let mut request = reqwest::Client::new()
            .post(format!("{}/token", self.base))
            .body(format!("grant_type=refresh_token&refresh_token={refresh}"));
        if let Some(basic) = basic {
            request = request.header("authorization", basic);
        }
        request.send().await.unwrap().status().is_success()
    }
}

/// A home of the test's own, under the test data folder.
fn home() -> PathBuf {
    let home = std::env::temp_dir().join(format!("mcp-auth-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn roots(home: &Path) -> Roots {
    Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap()
}

fn session(agent: Agent) -> Session<'static> {
    Session {
        agent,
        cwd: Path::new(""),
        project: None,
        in_checkout: agent == Agent::Codex,
    }
}

fn server<'a>(catalog: &'a Catalog, id: &str) -> &'a Server {
    catalog
        .server(id)
        .unwrap_or_else(|| panic!("{id} in the catalog"))
}

/// Claude Code's credentials (its Keychain item, else its file), as `memory` and `home` hold
/// them; reads of the item ask the user.
fn put_claude(memory: &Memory, roots: &Roots, blob: &Value) {
    match Homes::new(roots).claude_item {
        Some((service, account)) => {
            memory.others.lock().unwrap().insert(service.clone());
            memory.put(&service, &account, blob.to_string().as_bytes());
        }
        None => write(
            &roots.claude_dir.join(".credentials.json"),
            &blob.to_string(),
        ),
    }
}

fn claude_blob(memory: &Memory, roots: &Roots) -> Value {
    let bytes = match Homes::new(roots).claude_item {
        Some((service, account)) => memory.item(&service, &account).unwrap(),
        None => std::fs::read(roots.claude_dir.join(".credentials.json")).unwrap(),
    };
    serde_json::from_slice(&bytes).unwrap()
}

/// Logs written while it is the thread's subscriber.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn capture(&self) -> tracing::subscriber::DefaultGuard {
        let writer = self.clone();
        tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_max_level(tracing::Level::TRACE)
                .with_writer(move || writer.clone())
                .finish(),
        )
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

/// None of `secrets` is in `text`.
fn holds_none(what: &str, text: &str, secrets: &[String]) {
    for secret in secrets {
        assert!(
            !text.contains(secret.as_str()),
            "{what} holds a secret value"
        );
    }
}

fn auth(memory: Arc<Memory>, grant: &'static str) -> Arc<McpAuth> {
    Arc::new(McpAuth::new(
        memory,
        Arc::new(move |key: &str| key == grant),
    ))
}

/// The place whose stored sign-in serves `server` to `agent`'s workers, as a worker's start
/// reads it.
fn reused(catalog: &Catalog, server: &Server, agent: Agent, asking: &Prober<'_>) -> Option<Place> {
    match catalog.sign_in_from(server, &session(agent), asking)? {
        (SignIn::ReusableFrom(owner), Some(entry)) => asking.place(owner.into(), entry),
        (SignIn::InBrigadier, _) => asking.place(Owner::Brigadier, server),
        _ => None,
    }
}

async fn post(url: &str, key: Option<&str>) -> (StatusCode, String, bool) {
    let mut request = reqwest::Client::new()
        .post(url)
        .header("content-type", "application/json")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
    if let Some(key) = key {
        request = request.header("authorization", format!("Bearer {key}"));
    }
    let response = request.send().await.unwrap();
    let status = response.status();
    let challenged = response.headers().contains_key("www-authenticate");
    (status, response.text().await.unwrap(), challenged)
}

#[tokio::test]
async fn settings_never_asks_and_reports_a_reusable_sign_in_only_once_read() {
    let home = home();
    let url = "https://mcp.example.com/mcp";
    write(
        &home.join(".claude.json"),
        &json!({"mcpServers": {
            "linear": {"type": "http", "url": url},
            "notion": {"type": "http", "url": "https://notion.example.com/mcp"},
            "acme": {"type": "http", "url": url, "headers": {"X-Tenant": "acme"}},
        }})
        .to_string(),
    );
    write(
        &home.join(".codex/config.toml"),
        &format!(
            "[mcp_servers.globex]\nurl = \"{url}\"\nhttp_headers = {{ \"X-Tenant\" = \"globex\" }}\n\n[mcp_servers.acme2]\nurl = \"{url}\"\nhttp_headers = {{ \"x-tenant\" = \"acme\" }}\n"
        ),
    );
    let roots = roots(&home);
    let catalog = Catalog::read(&roots, None);
    let entry = |key: String, url: &str, refresh: Option<&str>, expires_at: u64| {
        let mut entry = json!({"serverName": key.split('|').next().unwrap(), "serverUrl": url,
                               "accessToken": "stored-access", "expiresAt": expires_at});
        if let Some(refresh) = refresh {
            entry["refreshToken"] = json!(refresh);
        }
        (key, entry)
    };
    let (linear, notion, acme) = (
        server(&catalog, "claude:user:linear"),
        server(&catalog, "claude:user:notion"),
        server(&catalog, "claude:user:acme"),
    );
    let entries: serde_json::Map<String, Value> = [
        entry(claude_key(linear).unwrap(), url, Some("stored-refresh"), 0),
        // Expired and without a refresh token: it can't serve.
        entry(
            claude_key(notion).unwrap(),
            "https://notion.example.com/mcp",
            None,
            1,
        ),
        entry(claude_key(acme).unwrap(), url, None, now_ms() + 3_600_000),
    ]
    .into_iter()
    .collect();
    let memory = Arc::new(Memory::default());
    put_claude(
        &memory,
        &roots,
        &json!({"claudeAiOauth": {"accessToken": "claude-ai-login"}, "mcpOAuth": entries}),
    );
    let auth = auth(memory.clone(), "worker");
    let quiet = auth.prober(&roots, catalog.codex_store, true);
    let asking = auth.prober(&roots, catalog.codex_store, false);
    let codex = session(Agent::Codex);
    let state = |server, stores: &Prober<'_>| catalog.sign_in(server, &codex, stores).unwrap();

    // Settings' reads never ask: Claude's item would, so its sign-in isn't known yet.
    if Homes::new(&roots).claude_item.is_some() {
        assert_eq!(state(linear, &quiet), SignIn::Unknown);
        assert!(memory.reads.lock().unwrap().iter().all(|(_, quiet)| *quiet));
        // Claude's own workers keep their own sign-in as it is.
        assert_eq!(
            catalog.sign_in(linear, &session(Agent::Claude), &quiet),
            Some(SignIn::Native)
        );
    }
    // A worker's read may ask; once read, Settings knows it without reading again.
    assert_eq!(state(linear, &asking), SignIn::ReusableFrom(Agent::Claude));
    let reads = memory.reads.lock().unwrap().len();
    assert_eq!(state(linear, &quiet), SignIn::ReusableFrom(Agent::Claude));
    assert_eq!(memory.reads.lock().unwrap().len(), reads);
    // A sign-in that can't serve, or none: the user signs in once.
    assert_eq!(state(notion, &asking), SignIn::Needed);
    // The same address for another tenant is another server.
    let (globex, acme2) = (
        server(&catalog, "codex:user:globex"),
        server(&catalog, "codex:user:acme2"),
    );
    assert_eq!(state(globex, &asking), SignIn::Needed);
    assert_eq!(state(acme2, &asking), SignIn::ReusableFrom(Agent::Claude));

    // The user says no: macOS wasn't allowed, for every sign-in in Claude's one item, and
    // nothing of it is kept. No read asks again until the user asks for it.
    let auth = self::auth(memory.clone(), "worker");
    memory.deny.store(true, Ordering::SeqCst);
    let asking = auth.prober(&roots, catalog.codex_store, false);
    if Homes::new(&roots).claude_item.is_some() {
        assert_eq!(state(linear, &asking), SignIn::NotAllowed(Agent::Claude));
        let claude_s = |place: &Place| matches!(place, Place::Claude { .. });
        assert!(!lock(&auth.known).keys().any(claude_s) && !lock(&auth.live).keys().any(claude_s));
        memory.deny.store(false, Ordering::SeqCst);
        let claude_reads = || {
            let (service, _) = Homes::new(&roots).claude_item.unwrap();
            let reads = memory.reads.lock().unwrap();
            reads.iter().filter(|(read, _)| *read == service).count()
        };
        let reads = claude_reads();
        assert_eq!(state(linear, &asking), SignIn::NotAllowed(Agent::Claude));
        assert_eq!(state(acme2, &asking), SignIn::NotAllowed(Agent::Claude));
        let quiet = auth.prober(&roots, catalog.codex_store, true);
        assert_eq!(state(linear, &quiet), SignIn::NotAllowed(Agent::Claude));
        assert_eq!(claude_reads(), reads);
        auth.ask_again(&[asking.place(Owner::Claude, linear).unwrap()]);
    }
    memory.deny.store(false, Ordering::SeqCst);
    assert_eq!(state(linear, &asking), SignIn::ReusableFrom(Agent::Claude));

    // Brigadier's own sign-in serves every agent; its reads never ask.
    let stores = auth.stores(Homes::new(&roots));
    let Some(Place::Brigadier { identity, url: own }) =
        Place::of(Owner::Brigadier, notion, CodexStore::Auto)
    else {
        panic!("Brigadier's place");
    };
    stores
        .save_own(
            &identity,
            &own,
            &Credential {
                access_token: Secret::new("own-access"),
                refresh_token: Some(Secret::new("own-refresh")),
                expires_at: Some(now_ms() + 3_600_000),
                client_id: Some("registered".into()),
                client_secret: None,
                token_endpoint: None,
                since: Some(1_000),
                found: Found::Own,
            },
        )
        .unwrap();
    let quiet = auth.prober(&roots, catalog.codex_store, true);
    assert_eq!(state(notion, &quiet), SignIn::InBrigadier);
    // A Claude worker's own entry can't serve once read, so Brigadier's serves it too.
    let asking = auth.prober(&roots, catalog.codex_store, false);
    assert_eq!(
        catalog.sign_in(notion, &session(Agent::Claude), &asking),
        Some(SignIn::InBrigadier)
    );
}

#[tokio::test]
async fn a_claude_sign_in_serves_a_codex_worker_past_expiry_and_stays_claudes() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let fake = Fake::start().await;
    let home = home();
    write(
        &home.join(".claude.json"),
        &json!({"mcpServers": {"linear": {"type": "http", "url": fake.url()}}}).to_string(),
    );
    let roots = roots(&home);
    let catalog = Catalog::read(&roots, None);
    let linear = server(&catalog, "claude:user:linear");
    let key = claude_key(linear).unwrap();
    let (access, refresh) = fake.signed_in();
    // Claude's item, with its own login and key beside the sign-in, and another server's.
    let blob = json!({
        "claudeAiOauth": {"accessToken": "claude-ai-login-secret", "refreshToken": "claude-ai-refresh-secret"},
        "primaryApiKey": "sk-ant-api-key-secret",
        "mcpOAuth": {
            "other|0123456789abcdef": {"serverName": "other", "serverUrl": "https://other.example.com/mcp",
                                        "accessToken": "other-access-secret", "expiresAt": 1},
            key.clone(): {"serverName": "linear", "serverUrl": fake.url(), "accessToken": access,
                          "refreshToken": refresh, "expiresAt": now_ms() - 1000,
                          "scope": "read", "clientId": "claude-client", "redirectUri": "http://localhost:1/callback",
                          "discoveryState": {"authorizationServerUrl": fake.base,
                                             "authorizationServerMetadata": {"token_endpoint": format!("{}/token", fake.base)}}},
        },
        "mcpOAuthClientConfig": {},
    });
    let memory = Arc::new(Memory::default());
    put_claude(&memory, &roots, &blob);
    let auth = auth(memory.clone(), "worker-key");
    let asking = auth.prober(&roots, catalog.codex_store, false);
    let place = reused(&catalog, linear, Agent::Codex, &asking).expect("Claude's sign-in");
    assert!(matches!(&place, Place::Claude { key: at, .. } if *at == key));
    let address = auth
        .route(Route {
            grant: "worker-key".into(),
            place,
            url: fake.url(),
            headers: Vec::new(),
            client: None,
            homes: asking.homes().clone(),
        })
        .await
        .unwrap();
    assert!(address.starts_with("http://127.0.0.1:"), "{address}");

    // Only the worker's own key gets through.
    for key in [None, Some("another-key")] {
        let (status, body, _) = post(&address, key).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        holds_none("a refusal", &body, &[access.clone(), "worker-key".into()]);
    }
    assert!(fake.state().seen.is_empty());

    // The stored token has expired: it is renewed before the request goes on.
    let (status, _, challenged) = post(&address, Some("worker-key")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!challenged);
    assert_eq!(fake.state().refreshes, 1);
    // Later the server stops taking it: the 401 renews it once and the request is replayed.
    fake.expire();
    let (status, _, _) = post(&address, Some("worker-key")).await;
    assert_eq!(status, StatusCode::OK);
    let (refreshes, seen, issued) = {
        let state = fake.state();
        (state.refreshes, state.seen.clone(), state.issued.clone())
    };
    assert_eq!(refreshes, 2);
    assert_eq!(seen.len(), 3, "{} requests reached the server", seen.len());
    assert!(seen.iter().all(|sent| sent.starts_with("Bearer access-")));

    // Claude's item holds the newest tokens, and nothing else in it changed.
    let now = claude_blob(&memory, &roots);
    let entry = &now["mcpOAuth"][&key];
    let (access, refresh) = (
        entry["accessToken"].as_str().unwrap().to_owned(),
        entry["refreshToken"].as_str().unwrap().to_owned(),
    );
    assert_eq!(seen.last().unwrap(), &format!("Bearer {access}"));
    assert!(entry["expiresAt"].as_u64().unwrap() > now_ms() + 3_000_000);
    let without_tokens = |mut blob: Value| {
        let entry = blob["mcpOAuth"][&key].as_object_mut().unwrap();
        for field in ["accessToken", "refreshToken", "expiresAt"] {
            entry.remove(field);
        }
        blob
    };
    assert_eq!(without_tokens(now), without_tokens(blob.clone()));
    // Claude goes on with it: its refresh token is the one the server takes now, the first
    // one was rotated away.
    assert!(fake.takes_refresh(&refresh).await);
    assert!(
        !fake
            .takes_refresh(blob["mcpOAuth"][&key]["refreshToken"].as_str().unwrap())
            .await
    );
    // The owner's lock is let go.
    assert!(
        !roots
            .claude_dir
            .join(format!(
                "{}.lock",
                brigadier_registry::claude_lock_name(&key)
            ))
            .exists()
    );

    let mut secrets = issued;
    secrets.extend(
        [
            "claude-ai-login-secret",
            "claude-ai-refresh-secret",
            "sk-ant-api-key-secret",
            "other-access-secret",
        ]
        .map(str::to_owned),
    );
    holds_none("the logs", &logs.text(), &secrets);
}

#[tokio::test]
async fn codex_s_file_store_and_configured_client_serve_a_claude_worker() {
    let logs = Logs::default();
    let _logging = logs.capture();
    let fake = Fake::start().await;
    fake.state().basic = Some(format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("codex-client:client-secret-value")
    ));
    let home = home();
    write(
        &home.join(".codex/config.toml"),
        &format!(
            "mcp_oauth_credentials_store = \"file\"\n\n[mcp_servers.linear]\nurl = \"{}\"\n\n[mcp_servers.linear.oauth]\nclient_id = \"codex-client\"\nclient_secret = \"client-secret-value\"\n",
            fake.url()
        ),
    );
    let roots = roots(&home);
    let catalog = Catalog::read(&roots, None);
    assert_eq!(catalog.codex_store, CodexStore::File);
    let linear = server(&catalog, "codex:user:linear");
    let key = codex_keys(linear)[0].clone();
    let (access, refresh) = fake.signed_in();
    let file = json!({key.clone(): {
        "server_name": "linear", "server_url": fake.url(), "client_id": "codex-client",
        "access_token": access, "expires_at": now_ms() - 1000, "refresh_token": refresh,
        "scopes": ["read"],
    }});
    write(&home.join(".codex/.credentials.json"), &file.to_string());
    // A Keychain item for the same server, which Codex doesn't read in file mode.
    let memory = Arc::new(Memory::default());
    let stale = json!({"server_name": "linear", "url": fake.url(), "client_id": "codex-client",
                       "token_response": {"access_token": "keychain-access-secret",
                                          "refresh_token": "keychain-refresh-secret"},
                       "expires_at": now_ms() + 3_600_000})
    .to_string();
    memory.put("Codex MCP Credentials", &key, stale.as_bytes());
    let auth = auth(memory.clone(), "worker-key");
    let asking = auth.prober(&roots, catalog.codex_store, false);
    assert_eq!(
        catalog.sign_in(linear, &session(Agent::Claude), &asking),
        Some(SignIn::ReusableFrom(Agent::Codex))
    );
    let place = reused(&catalog, linear, Agent::Claude, &asking).unwrap();
    let address = auth
        .route(Route {
            grant: "worker-key".into(),
            place,
            url: fake.url(),
            headers: Vec::new(),
            client: linear.oauth.clone(),
            homes: asking.homes().clone(),
        })
        .await
        .unwrap();

    // Codex's file has no token endpoint: it is found from the server's challenge, and the
    // refresh authenticates with the configured client.
    let (status, _, _) = post(&address, Some("worker-key")).await;
    assert_eq!(status, StatusCode::OK);
    let (refreshes, seen, issued) = {
        let state = fake.state();
        (state.refreshes, state.seen.clone(), state.issued.clone())
    };
    assert_eq!(refreshes, 1);
    assert!(
        !seen
            .iter()
            .any(|sent| sent.contains("keychain-access-secret"))
    );
    let now: Value =
        serde_json::from_slice(&std::fs::read(home.join(".codex/.credentials.json")).unwrap())
            .unwrap();
    let entry = &now[&key];
    assert_eq!(
        seen.last().unwrap(),
        &format!("Bearer {}", entry["access_token"].as_str().unwrap())
    );
    assert_eq!(entry["scopes"], json!(["read"]));
    assert_eq!(entry["client_id"], "codex-client");
    assert!(
        fake.takes_refresh(entry["refresh_token"].as_str().unwrap())
            .await
    );
    // The Keychain item is as it was, and Codex's lock files are where Codex takes them.
    assert_eq!(
        memory.item("Codex MCP Credentials", &key).unwrap(),
        stale.as_bytes()
    );
    assert!(
        home.join(".codex/mcp-oauth-locks")
            .join(format!("{}.lock", sha256_hex(&key)))
            .exists()
    );
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(home.join(".codex/.credentials.json"))
                .unwrap()
                .permissions()
        ) & 0o777,
        0o600
    );
    let mut secrets = issued;
    secrets.extend(["client-secret-value", "keychain-access-secret"].map(str::to_owned));
    holds_none("the logs", &logs.text(), &secrets);
}

#[tokio::test]
async fn an_owner_s_own_renewal_is_adopted_and_a_refused_one_ends_the_reuse() {
    let fake = Fake::start().await;
    let home = home();
    write(
        &home.join(".codex/config.toml"),
        &format!(
            "mcp_oauth_credentials_store = \"file\"\n\n[mcp_servers.linear]\nurl = \"{}\"\n",
            fake.url()
        ),
    );
    let roots = roots(&home);
    let catalog = Catalog::read(&roots, None);
    let linear = server(&catalog, "codex:user:linear");
    let key = codex_keys(linear)[0].clone();
    let store = |access: &str, refresh: &str| {
        let file = json!({key.clone(): {
            "server_name": "linear", "server_url": fake.url(), "access_token": access,
            "expires_at": now_ms() + 3_600_000, "refresh_token": refresh,
        }});
        write(&home.join(".codex/.credentials.json"), &file.to_string());
    };
    let (access, refresh) = fake.signed_in();
    store(&access, &refresh);
    let memory = Arc::new(Memory::default());
    let auth = auth(memory, "worker-key");
    let asking = auth.prober(&roots, catalog.codex_store, false);
    let address = auth
        .route(Route {
            grant: "worker-key".into(),
            place: reused(&catalog, linear, Agent::Claude, &asking).unwrap(),
            url: fake.url(),
            headers: Vec::new(),
            client: None,
            homes: asking.homes().clone(),
        })
        .await
        .unwrap();
    assert_eq!(post(&address, Some("worker-key")).await.0, StatusCode::OK);

    // Codex renewed it meanwhile: Brigadier takes Codex's new token, refreshing nothing.
    fake.expire();
    let (access, refresh) = fake.signed_in();
    store(&access, &refresh);
    assert_eq!(post(&address, Some("worker-key")).await.0, StatusCode::OK);
    assert_eq!(fake.state().refreshes, 0);
    assert_eq!(
        fake.state().seen.last().unwrap(),
        &format!("Bearer {access}")
    );

    // The server refuses the refresh token: the sign-in is no longer reused, and the user is
    // asked to sign in again.
    fake.expire();
    fake.state().refresh.clear();
    let (status, body, _) = post(&address, Some("worker-key")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    holds_none("a refusal", &body, &[access.clone(), refresh.clone()]);
    assert_eq!(
        catalog.sign_in(linear, &session(Agent::Claude), &asking),
        Some(SignIn::Needed)
    );
    // Codex's own file is left as it was.
    let now: Value =
        serde_json::from_slice(&std::fs::read(home.join(".codex/.credentials.json")).unwrap())
            .unwrap();
    assert_eq!(now[&key]["refresh_token"], refresh.as_str());
    // A new sign-in in Codex is reused again, once what was read of the store is old.
    let (access, refresh) = fake.signed_in();
    store(&access, &refresh);
    lock(&auth.known).clear();
    assert_eq!(
        catalog.sign_in(linear, &session(Agent::Claude), &asking),
        Some(SignIn::ReusableFrom(Agent::Codex))
    );
}

/// Waits until `done`, for at most 10 s.
async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..1000 {
        if done() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("{what} didn't happen");
}

/// A link's query value `name`.
fn query(link: &str, name: &str) -> String {
    url::Url::parse(link)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .unwrap()
        .1
        .into_owned()
}

#[tokio::test]
async fn brigadier_s_own_sign_in_reopens_cancels_fails_plainly_and_signs_out() {
    use crate::manager::integrations::{SignInState, view};
    use crate::model::Settings;

    let logs = Logs::default();
    let _logging = logs.capture();
    let fake = Fake::start().await;
    let home = home();
    write(
        &home.join(".claude.json"),
        &json!({"mcpServers": {"linear": {"type": "http", "url": fake.url()}}}).to_string(),
    );
    let roots = roots(&home);
    let catalog = Catalog::read(&roots, None);
    let linear = server(&catalog, "claude:user:linear").clone();
    let memory = Arc::new(Memory::default());
    let auth = auth(memory.clone(), "worker");
    let place = Place::of(Owner::Brigadier, &linear, CodexStore::Auto).unwrap();
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let open: Open = {
        let opened = opened.clone();
        Arc::new(move |link: &str| opened.lock().unwrap().push(link.to_owned()))
    };
    let start = || {
        let (id, cancelled) = auth.begin_sign_in(place.clone(), open.clone());
        let (auth, server, homes) = (auth.clone(), linear.clone(), Homes::new(&roots));
        let running = id.clone();
        tokio::spawn(async move {
            auth.sign_in(&running, &server, homes, cancelled.unwrap())
                .await;
        });
        id
    };
    let opened_count = || opened.lock().unwrap().len();
    let last_link = || opened.lock().unwrap().last().unwrap().clone();
    let shown = || {
        let quiet = auth.prober(&roots, catalog.codex_store, true);
        view(&catalog, None, &Settings::default(), &quiet)
            .servers
            .into_iter()
            .find(|server| server.id == linear.id)
            .unwrap()
    };
    let own_items = || {
        memory
            .items
            .lock()
            .unwrap()
            .keys()
            .filter(|(service, _)| service == stores::OWN_SERVICE)
            .count()
    };

    // Waiting for the browser: the catalog shows it, asking again opens the same page and
    // gives the same sign-in, and its page opens again on request.
    let id = start();
    until("the sign-in page to open", || opened_count() == 1).await;
    assert_eq!(shown().pending_sign_in.as_deref(), Some(id.as_str()));
    let (again, cancelled) = auth.begin_sign_in(place.clone(), open.clone());
    assert_eq!((again.as_str(), cancelled.is_none()), (id.as_str(), true));
    auth.reopen_sign_in(&id).unwrap();
    assert_eq!(opened_count(), 3);
    let link = last_link();
    assert!(opened.lock().unwrap().iter().all(|opened| *opened == link));

    // Cancelled: its callback listener closes and nothing is kept.
    let redirect = query(&last_link(), "redirect_uri");
    auth.cancel_sign_in(&id).unwrap();
    assert_eq!(auth.sign_in_status(&id), Some(McpSignInStatus::Cancelled));
    for _ in 0..1000 {
        if reqwest::get(&redirect).await.is_err() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        reqwest::get(&redirect).await.is_err(),
        "the listener is closed"
    );
    assert!(auth.reopen_sign_in(&id).is_err());
    assert_eq!(own_items(), 0);
    assert_eq!(shown().pending_sign_in, None);

    // Refused in the browser: a plain reason.
    let id = start();
    until("the sign-in page to open", || opened_count() == 4).await;
    let link = last_link();
    reqwest::get(format!(
        "{}?error=access_denied&state={}",
        query(&link, "redirect_uri"),
        query(&link, "state")
    ))
    .await
    .unwrap();
    until("the sign-in to end", || {
        auth.sign_in_status(&id) != Some(McpSignInStatus::Pending)
    })
    .await;
    assert_eq!(
        auth.sign_in_status(&id),
        Some(McpSignInStatus::Failed {
            reason: "The server didn't accept the sign-in.".into()
        })
    );

    // Signed in: it serves every agent, from the day it was made.
    let id = start();
    until("the sign-in page to open", || opened_count() == 5).await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let answer = http.get(last_link()).send().await.unwrap();
    let back = answer.headers()["location"].to_str().unwrap().to_owned();
    http.get(back).send().await.unwrap();
    until("the sign-in to end", || {
        auth.sign_in_status(&id) != Some(McpSignInStatus::Pending)
    })
    .await;
    assert_eq!(auth.sign_in_status(&id), Some(McpSignInStatus::SignedIn));
    assert_eq!(own_items(), 1);
    let shown_now = shown();
    let sign_in = shown_now.sign_in.unwrap();
    let since = now_ms();
    for state in [sign_in.claude, sign_in.codex] {
        let SignInState::InBrigadier {
            since_ms: Some(made),
        } = state
        else {
            panic!("{state:?}");
        };
        assert!(made <= since as i64 && made > since as i64 - 60_000);
    }
    assert_eq!(shown_now.pending_sign_in, None);

    // Signed out: only Brigadier's own item goes, and the user is asked to sign in again.
    auth.sign_out(&place, Homes::new(&roots)).await.unwrap();
    assert_eq!(own_items(), 0);
    assert_eq!(auth.signed_in_at(&place), None);
    let sign_in = shown().sign_in.unwrap();
    assert_eq!(
        (sign_in.claude, sign_in.codex),
        (SignInState::Needed, SignInState::Needed)
    );
    // Nothing to sign out of: nothing happens.
    auth.sign_out(&place, Homes::new(&roots)).await.unwrap();

    let issued = fake.state().issued.clone();
    assert!(!issued.is_empty());
    holds_none("the logs", &logs.text(), &issued);
    holds_none(
        "the sign-in pages",
        &opened.lock().unwrap().join("\n"),
        &issued,
    );
}

#[tokio::test]
async fn without_a_keychain_sign_ins_are_unsupported() {
    use crate::manager::integrations::{SignInState, view};
    use crate::model::Settings;

    let home = home();
    write(
        &home.join(".claude.json"),
        &json!({"mcpServers": {"linear": {"type": "http", "url": "https://mcp.example.com/mcp"}}})
            .to_string(),
    );
    let roots = roots(&home);
    let catalog = Catalog::read(&roots, None);
    let auth = Arc::new(McpAuth::new(
        Arc::new(keychain::Missing),
        Arc::new(|_: &str| false),
    ));
    assert!(!auth.keeps_sign_ins());
    let quiet = auth.prober(&roots, catalog.codex_store, true);
    let shown = view(&catalog, None, &Settings::default(), &quiet);
    let sign_in = shown.servers[0].sign_in.clone().unwrap();
    // Claude's workers keep their own; Codex's can't be checked here, which isn't "not read
    // yet".
    assert_eq!(
        (sign_in.claude, sign_in.codex),
        (SignInState::Native, SignInState::Unsupported)
    );
}

mod in_brigadier {
    //! The sign-in in Brigadier, through the manager the IPC methods call, serving workers of
    //! both agents whose own configs have the server unsigned.

    use std::sync::Arc;

    use brigadier_providers::ProviderKind;
    use brigadier_providers::model::{McpTransport, SessionSpec};
    use brigadier_registry::Roots;
    use serde_json::json;

    use super::{Fake, Logs, holds_none, post, write};
    use crate::manager::McpSignInStatus;
    use crate::manager::flow::{Flow, Options, Reply, Script, eventually_async};
    use crate::manager::integrations::SignInState;
    use crate::manager::mcp_auth::Memory;
    use crate::manager::mcp_auth::stores::OWN_SERVICE;

    /// The thread delegates one task to each agent on "Go."; each worker waits for `go` before
    /// it reports.
    fn script(go: Arc<tokio::sync::Semaphore>) -> Script {
        Arc::new(move |turn| {
            let go = go.clone();
            Box::pin(async move {
                if turn.is_orchestrator() {
                    if !turn.input.contains("Go.") {
                        return Reply::text("Noted.");
                    }
                    for provider in ["claude", "codex"] {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "low", "title": format!("Look around in {provider}"),
                                       "kind": "research", "spec": "Look around.",
                                       "provider": provider, "integrations": []}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                go.acquire().await.unwrap().forget();
                let reply = turn
                    .call("submit_report", json!({"summary": "Looked around."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            })
        })
    }

    fn workers(flow: &Flow) -> Vec<(ProviderKind, SessionSpec)> {
        flow.specs()
            .into_iter()
            .filter(|(_, spec)| {
                spec.append_system_prompt.as_deref().is_some_and(|prompt| {
                    !prompt.contains(crate::manager::prompts::THREAD_OPENING)
                        && prompt != brigadier_review::REVIEW_ROLE
                })
            })
            .collect()
    }

    /// A browser that follows the sign-in link and the authorization server's redirect.
    fn browser() -> Arc<dyn Fn(&str) + Send + Sync> {
        Arc::new(|link: &str| {
            let link = link.to_owned();
            tokio::spawn(async move {
                let http = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .build()
                    .unwrap();
                let answer = http.get(&link).send().await.unwrap();
                let back = answer.headers()["location"].to_str().unwrap().to_owned();
                http.get(back).send().await.unwrap();
            });
        })
    }

    async fn signed_in(flow: &Flow, id: &str) {
        eventually_async("the sign-in to finish", || async {
            flow.manager.mcp_sign_in(id).unwrap() != McpSignInStatus::Pending
        })
        .await;
        assert_eq!(
            flow.manager.mcp_sign_in(id).unwrap(),
            McpSignInStatus::SignedIn
        );
    }

    #[tokio::test]
    async fn one_sign_in_in_brigadier_serves_both_agents_workers() {
        let logs = Logs::default();
        let _logging = logs.capture();
        let fake = Fake::start().await;
        let go = Arc::new(tokio::sync::Semaphore::new(0));
        let flow = Flow::start(
            "mcp-sign-in-brigadier",
            Options::default(),
            script(go.clone()),
        )
        .await;
        let home = flow.repo.parent().unwrap().join("home");
        write(
            &home.join(".claude.json"),
            &json!({"mcpServers": {"linear": {"type": "http", "url": fake.url()}}}).to_string(),
        );
        write(
            &home.join(".codex/config.toml"),
            &format!("[mcp_servers.linear]\nurl = \"{}\"\n", fake.url()),
        );
        flow.manager.set_registry(
            Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap(),
        );
        let memory = Arc::new(Memory::default());
        flow.manager.mcp_auth.set_keychain(memory.clone());
        let states = || async {
            let view = flow.manager.integrations_view(None).await.unwrap();
            let linear = view
                .servers
                .iter()
                .find(|server| server.id == "claude:user:linear")
                .unwrap()
                .clone();
            let sign_in = linear.sign_in.unwrap();
            (sign_in.claude, sign_in.codex)
        };
        // Neither agent is signed in: the user is asked to sign in.
        assert_eq!(states().await, (SignInState::Needed, SignInState::Needed));

        let id = flow
            .manager
            .sign_in_mcp_server("claude:user:linear", None, browser())
            .await
            .unwrap();
        signed_in(&flow, &id).await;
        let (claude, codex) = states().await;
        for state in [claude, codex] {
            assert!(
                matches!(state, SignInState::InBrigadier { since_ms: Some(_) }),
                "{state:?}"
            );
        }
        assert_eq!(
            memory
                .items
                .lock()
                .unwrap()
                .keys()
                .filter(|(service, _)| service == OWN_SERVICE)
                .count(),
            1
        );

        flow.say("Go.").await;
        eventually_async("both workers to start", || async {
            workers(&flow).len() == 2
        })
        .await;
        let workers = workers(&flow);
        let spec = |provider| {
            &workers
                .iter()
                .find(|(kind, _)| *kind == provider)
                .unwrap()
                .1
        };
        let (claude, codex) = (spec(ProviderKind::Claude), spec(ProviderKind::Codex));
        // Codex's own entry gives way to the one Brigadier serves under its name; Claude goes
        // without its own and gets the served one under another.
        let served = |spec: &SessionSpec, name: &str| {
            let server = spec
                .mcp_servers
                .iter()
                .find(|server| server.name == name)
                .unwrap_or_else(|| panic!("{name}"));
            let McpTransport::Http { url } = &server.transport else {
                panic!("{name} is remote");
            };
            assert!(url.starts_with("http://127.0.0.1:"), "{url}");
            let [(header, value)] = server.env.as_slice() else {
                panic!("one header");
            };
            assert_eq!(header, "Authorization");
            (
                url.clone(),
                value.strip_prefix("Bearer ").unwrap().to_owned(),
            )
        };
        let codex_route = served(codex, "linear");
        assert!(claude.switched_off.servers.contains(&"linear".to_owned()));
        let claude_route = served(claude, "linear-2");
        for (url, key) in [&codex_route, &claude_route] {
            assert_eq!(post(url, Some(key)).await.0, hyper::StatusCode::OK);
        }
        // Each route takes its own worker's key only.
        assert_eq!(
            post(&codex_route.0, Some(&claude_route.1)).await.0,
            hyper::StatusCode::UNAUTHORIZED
        );
        go.add_permits(2);
        flow.settled().await;

        // No token reached the logs, the workers' sessions, Settings' view, the sign-in's
        // answers or Brigadier's data folder.
        let issued = fake.state().issued.clone();
        assert!(!issued.is_empty());
        holds_none("the logs", &logs.text(), &issued);
        holds_none("the workers' sessions", &format!("{workers:?}"), &issued);
        let view = flow.manager.integrations_view(None).await.unwrap();
        holds_none(
            "Settings' view",
            &serde_json::to_string(&view).unwrap(),
            &issued,
        );
        holds_none(
            "the sign-in's answer",
            &serde_json::to_string(&flow.manager.mcp_sign_in(&id).unwrap()).unwrap(),
            &issued,
        );
        for file in walk(&flow.manager.data_dir) {
            let bytes = std::fs::read(&file).unwrap_or_default();
            holds_none(
                &file.display().to_string(),
                &String::from_utf8_lossy(&bytes),
                &issued,
            );
        }
        flow.stop().await;
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => files.extend(walk(&path)),
                Ok(kind) if kind.is_file() => files.push(path),
                _ => {}
            }
        }
        files
    }

    #[tokio::test]
    async fn a_project_s_server_signs_in_only_in_a_trusted_project() {
        let fake = Fake::start().await;
        let flow = Flow::start(
            "mcp-sign-in-project",
            Options::default(),
            script(Arc::new(tokio::sync::Semaphore::new(0))),
        )
        .await;
        let home = flow.repo.parent().unwrap().join("home");
        std::fs::create_dir_all(&home).unwrap();
        write(
            &flow.repo.join(".mcp.json"),
            &json!({"mcpServers": {"tracker": {"type": "http", "url": fake.url()}}}).to_string(),
        );
        flow.manager.set_registry(
            Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap(),
        );
        let id = flow
            .manager
            .registry()
            .catalog(Some(&flow.repo))
            .servers
            .iter()
            .find(|server| server.name == "tracker")
            .unwrap()
            .id
            .clone();
        let project = flow
            .core
            .conversation(&flow.conversation)
            .unwrap()
            .project_id
            .unwrap();
        // Without its project it isn't found.
        let err = flow
            .manager
            .sign_in_mcp_server(&id, None, browser())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no MCP server"), "{err}");
        let signing = flow
            .manager
            .sign_in_mcp_server(&id, Some(project.clone()), browser())
            .await
            .unwrap();
        signed_in(&flow, &signing).await;

        let repo = flow.repo.display().to_string();
        flow.core
            .set_folder_trust(&project, &repo, false)
            .await
            .unwrap();
        let err = flow
            .manager
            .sign_in_mcp_server(&id, Some(project), browser())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("isn't trusted"), "{err}");
        flow.stop().await;
    }
}
