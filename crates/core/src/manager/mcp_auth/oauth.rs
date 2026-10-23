//! OAuth for remote MCP servers (the MCP authorization spec, OAuth 2.1): finding a server's
//! authorization server, renewing a sign-in with its refresh token, and signing in once in
//! Brigadier with PKCE, dynamic client registration and a loopback redirect. An error is a short
//! plain sentence for the user, its technical cause only logged; no token value is ever logged
//! or put in an error.

use std::time::Duration;

use base64::Engine as _;
use brigadier_registry::{OAuthClient, Secret};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

use super::stores::{Credential, Found, now_ms};

/// How long one request to an authorization server may take.
const TIMEOUT: Duration = Duration::from_secs(20);
/// How long the user has to finish signing in in their browser.
const SIGN_IN_WAIT: Duration = Duration::from_secs(300);

/// Why a renewal or a sign-in failed: never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OAuthError {
    /// The authorization server refused the grant (`invalid_grant`, or the user declined in
    /// the browser): a renewed sign-in is over.
    Refused,
    /// The user didn't finish signing in in time.
    TimedOut,
    /// Why else, as a short plain sentence.
    Failed(String),
}

impl std::fmt::Display for OAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused => f.write_str("The server didn't accept the sign-in."),
            Self::TimedOut => f.write_str("Timed out after 5 minutes. Nothing was saved."),
            Self::Failed(reason) => f.write_str(reason),
        }
    }
}

/// `plain`, the user's reason, for `err`, which is only logged.
fn failed(plain: &str, err: impl std::fmt::Display) -> OAuthError {
    tracing::info!(error = %err, "{plain}");
    OAuthError::Failed(plain.to_owned())
}

/// The plain reason when the server can't be reached.
const UNREACHABLE: &str = "The server couldn't be reached.";
/// The plain reason when the sign-in can't wait for the browser.
const NOT_STARTED: &str = "The sign-in couldn't be started.";

/// What a server's authorization server says of itself (RFC 8414).
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct AuthServer {
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
}

/// A server's protected resource metadata (RFC 9728).
#[derive(Debug, Deserialize)]
struct Resource {
    resource: Option<String>,
    #[serde(default)]
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}

/// Where a server signs in.
#[derive(Debug, Clone)]
pub(crate) struct Discovered {
    pub auth: AuthServer,
    /// The resource its tokens are for (RFC 8707), sent with each token request.
    pub resource: String,
    pub scopes: Vec<String>,
}

/// The HTTP client for OAuth and the proxy: no redirects followed.
pub(crate) fn client() -> reqwest::Client {
    // reqwest is built without a crypto provider of its own; ring is the one in the build.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(TIMEOUT)
        .build()
        .unwrap_or_default()
}

/// `resource_metadata` of a `WWW-Authenticate: Bearer …` challenge.
fn resource_metadata(challenge: &str) -> Option<String> {
    let at = challenge.find("resource_metadata=")? + "resource_metadata=".len();
    let rest = &challenge[at..];
    let value = match rest.strip_prefix('"') {
        Some(quoted) => &quoted[..quoted.find('"')?],
        None => rest.split([',', ' ']).next()?,
    };
    Some(value.to_owned())
}

/// `base` with `/.well-known/<name>` put before its path (RFC 8414 §3.1), and without.
fn well_known(base: &Url, name: &str) -> Vec<String> {
    let path = base.path().trim_end_matches('/');
    let origin = base.origin().ascii_serialization();
    let mut urls = Vec::new();
    if !path.is_empty() {
        urls.push(format!("{origin}/.well-known/{name}{path}"));
    }
    urls.push(format!("{origin}/.well-known/{name}"));
    urls
}

async fn get_json<T: for<'de> Deserialize<'de>>(http: &reqwest::Client, url: &str) -> Option<T> {
    let response = http
        .get(url)
        .header(ACCEPT, "application/json")
        .timeout(TIMEOUT)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    serde_json::from_slice(&response.bytes().await.ok()?).ok()
}

/// Finds where the server at `server_url` signs in: the resource metadata its 401 names
/// (else its well-known address), then its authorization server's metadata. `headers`: the
/// server's own non-secret headers, which some servers need to answer at all.
pub(crate) async fn discover(
    http: &reqwest::Client,
    server_url: &str,
    headers: &[(String, Secret)],
) -> Result<Discovered, OAuthError> {
    let server =
        Url::parse(server_url).map_err(|err| failed("The server's address isn't valid.", err))?;
    let mut probe = http
        .post(server.clone())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .timeout(TIMEOUT)
        .body(
            r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"Brigadier","version":"1"}}}"#,
        );
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("authorization") {
            probe = probe.header(name.as_str(), value.expose());
        }
    }
    let challenge = probe.send().await.ok().and_then(|response| {
        response
            .headers()
            .get_all(WWW_AUTHENTICATE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(resource_metadata)
    });
    let mut metadata_urls: Vec<String> = challenge.into_iter().collect();
    metadata_urls.extend(well_known(&server, "oauth-protected-resource"));
    let mut resource = None;
    for url in &metadata_urls {
        if let Some(found) = get_json::<Resource>(http, url).await {
            resource = Some(found);
            break;
        }
    }
    // A server without resource metadata (the 2025-03-26 spec) is its own authorization
    // server.
    let issuer = resource
        .as_ref()
        .and_then(|resource| resource.authorization_servers.first())
        .map_or_else(|| server.origin().ascii_serialization(), Clone::clone);
    let issuer = Url::parse(&issuer)
        .map_err(|err| failed("The server names a sign-in address that isn't valid.", err))?;
    let mut auth = None;
    for name in ["oauth-authorization-server", "openid-configuration"] {
        for url in well_known(&issuer, name) {
            if let Some(found) = get_json::<AuthServer>(http, &url).await {
                auth = Some(found);
                break;
            }
        }
        if auth.is_some() {
            break;
        }
    }
    if auth.is_none() && !issuer.path().trim_end_matches('/').is_empty() {
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.as_str().trim_end_matches('/')
        );
        auth = get_json::<AuthServer>(http, &url).await;
    }
    let auth = auth.ok_or_else(|| {
        OAuthError::Failed("Brigadier couldn't find where this server signs in.".into())
    })?;
    let (resource, scopes) = match resource {
        Some(found) => (
            found.resource.unwrap_or_else(|| server_url.to_owned()),
            found.scopes_supported,
        ),
        None => (server_url.to_owned(), Vec::new()),
    };
    Ok(Discovered {
        auth,
        resource,
        scopes,
    })
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct ErrorAnswer {
    error: String,
}

/// Posts `form` to `endpoint` with the client's authentication: its secret by HTTP Basic,
/// else its id in the form.
async fn token_request(
    http: &reqwest::Client,
    endpoint: &str,
    mut form: Vec<(&str, String)>,
    client_id: Option<&str>,
    client_secret: Option<&Secret>,
) -> Result<TokenAnswer, OAuthError> {
    let mut request = http
        .post(endpoint)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .timeout(TIMEOUT);
    match (client_id, client_secret) {
        (Some(id), Some(secret)) => {
            let encode = |text: &str| {
                url::form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>()
            };
            let basic = base64::engine::general_purpose::STANDARD.encode(format!(
                "{}:{}",
                encode(id),
                encode(secret.expose())
            ));
            request = request.header(AUTHORIZATION, format!("Basic {basic}"));
        }
        (Some(id), None) => form.push(("client_id", id.to_owned())),
        _ => {}
    }
    let body: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form.iter().map(|(name, value)| (*name, value.as_str())))
        .finish();
    let response = request
        .body(body)
        .send()
        .await
        .map_err(|err| failed(UNREACHABLE, err.without_url()))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|err| failed(UNREACHABLE, err.without_url()))?;
    if !status.is_success() {
        let error = serde_json::from_slice::<ErrorAnswer>(&bytes).map(|answer| answer.error);
        return Err(match error {
            Ok(error) if error == "invalid_grant" => OAuthError::Refused,
            Ok(error) => {
                OAuthError::Failed(format!("The server answered with an error ({error})."))
            }
            Err(_) => OAuthError::Failed(format!(
                "The server answered with an error ({}).",
                status.as_u16()
            )),
        });
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| OAuthError::Failed("The server's answer wasn't understood.".into()))
}

/// When a token that lives `expires_in` seconds from now expires (ms since the epoch).
fn expiry(expires_in: Option<u64>) -> Option<u64> {
    expires_in.map(|secs| now_ms().saturating_add(secs.saturating_mul(1000)))
}

fn random(bytes: usize) -> Result<String, OAuthError> {
    let mut buffer = vec![0u8; bytes];
    getrandom::fill(&mut buffer).map_err(|err| failed(NOT_STARTED, err))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer))
}

#[derive(Deserialize)]
struct Registered {
    client_id: String,
    client_secret: Option<String>,
}

/// Registers Brigadier as a client of `auth` (RFC 7591) for `redirect`.
async fn register(
    http: &reqwest::Client,
    endpoint: &str,
    redirect: &str,
) -> Result<OAuthClient, OAuthError> {
    let body = serde_json::json!({
        "client_name": "Brigadier",
        "redirect_uris": [redirect],
        "grant_types": ["authorization_code", "refresh_token"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
    });
    let response = http
        .post(endpoint)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .timeout(TIMEOUT)
        .body(body.to_string())
        .send()
        .await
        .map_err(|err| failed(UNREACHABLE, err.without_url()))?;
    let status = response.status();
    if !status.is_success() {
        tracing::info!(
            status = status.as_u16(),
            "the client registration was refused"
        );
        return Err(OAuthError::Failed(
            "The server didn't let Brigadier register to sign in.".into(),
        ));
    }
    let registered: Registered = serde_json::from_slice(
        &response
            .bytes()
            .await
            .map_err(|err| failed(UNREACHABLE, err.without_url()))?,
    )
    .map_err(|_| OAuthError::Failed("The server's answer wasn't understood.".into()))?;
    Ok(OAuthClient {
        client_id: Some(registered.client_id),
        client_secret: registered
            .client_secret
            .filter(|secret| !secret.is_empty())
            .map(Secret::new),
    })
}

/// The `code` the browser brings back to `listener` for `state`: the first request to
/// `/callback` carrying `state`. The browser's tab is told how it went.
async fn callback(listener: tokio::net::TcpListener, state: &str) -> Result<String, OAuthError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|err| failed(NOT_STARTED, err))?;
        let mut head = Vec::new();
        let mut buffer = [0u8; 2048];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
            match stream.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => head.extend_from_slice(&buffer[..read]),
            }
        }
        let line = String::from_utf8_lossy(&head);
        let target = line
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("GET "))
            .and_then(|rest| rest.split(' ').next())
            .unwrap_or_default()
            .to_owned();
        let query: Vec<(String, String)> = Url::parse(&format!("http://127.0.0.1{target}"))
            .ok()
            .filter(|url| url.path() == "/callback")
            .map(|url| url.query_pairs().into_owned().collect())
            .unwrap_or_default();
        let value = |name: &str| {
            query
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        if value("state").as_deref() != Some(state) {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await;
            continue;
        }
        let outcome = match (value("code"), value("error")) {
            (Some(code), _) => Ok(code),
            (None, Some(error)) => {
                tracing::info!(%error, "the sign-in was refused in the browser");
                Err(OAuthError::Refused)
            }
            (None, None) => Err(OAuthError::Failed(
                "The browser brought no sign-in back.".into(),
            )),
        };
        let page = if outcome.is_ok() {
            "Signed in. You can close this tab and go back to Brigadier."
        } else {
            "The sign-in didn't finish. You can close this tab and try again in Brigadier."
        };
        let _ = stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                )
                .as_bytes(),
            )
            .await;
        return outcome;
    }
}

/// Signs in to the server at `server_url` in the user's browser (`open` opens a link):
/// discovery, the configured client or a registered one, PKCE (S256), a loopback redirect on
/// 127.0.0.1, then the code's exchange. The credential is Brigadier's own.
pub(crate) async fn sign_in(
    http: &reqwest::Client,
    server_url: &str,
    headers: &[(String, Secret)],
    configured: Option<&OAuthClient>,
    open: &(dyn Fn(&str) + Send + Sync),
) -> Result<Credential, OAuthError> {
    let found = discover(http, server_url, headers).await?;
    let authorize = found
        .auth
        .authorization_endpoint
        .clone()
        .ok_or_else(|| OAuthError::Failed("The server has no sign-in page.".into()))?;
    if !found.auth.code_challenge_methods_supported.is_empty()
        && !found
            .auth
            .code_challenge_methods_supported
            .iter()
            .any(|method| method == "S256")
    {
        return Err(OAuthError::Failed(
            "The server's sign-in doesn't support PKCE, which Brigadier needs.".into(),
        ));
    }
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|err| failed(NOT_STARTED, err))?;
    let port = listener
        .local_addr()
        .map_err(|err| failed(NOT_STARTED, err))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let client = match configured.filter(|client| client.client_id.is_some()) {
        Some(client) => client.clone(),
        None => {
            let endpoint = found.auth.registration_endpoint.as_deref().ok_or_else(|| {
                OAuthError::Failed(
                    "The server doesn't let new apps sign in, and its config names no client."
                        .into(),
                )
            })?;
            register(http, endpoint, &redirect).await?
        }
    };
    let client_id = client.client_id.clone().unwrap_or_default();
    let verifier = random(32)?;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    let state = random(16)?;
    let mut link = Url::parse(&authorize)
        .map_err(|err| failed("The server's sign-in page isn't valid.", err))?;
    {
        let mut query = link.query_pairs_mut();
        query
            .append_pair("response_type", "code")
            .append_pair("client_id", &client_id)
            .append_pair("redirect_uri", &redirect)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("resource", &found.resource);
        if !found.scopes.is_empty() {
            query.append_pair("scope", &found.scopes.join(" "));
        }
    }
    open(link.as_str());
    let code = tokio::time::timeout(SIGN_IN_WAIT, callback(listener, &state))
        .await
        .map_err(|_| OAuthError::TimedOut)??;
    let answer = token_request(
        http,
        &found.auth.token_endpoint,
        vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code),
            ("redirect_uri", redirect),
            ("code_verifier", verifier),
            ("resource", found.resource.clone()),
        ],
        Some(&client_id),
        client.client_secret.as_ref(),
    )
    .await?;
    Ok(Credential {
        access_token: Secret::new(answer.access_token),
        refresh_token: answer
            .refresh_token
            .filter(|token| !token.is_empty())
            .map(Secret::new),
        expires_at: expiry(answer.expires_in),
        client_id: Some(client_id),
        client_secret: client.client_secret,
        token_endpoint: Some(found.auth.token_endpoint),
        since: Some(now_ms()),
        found: Found::Own,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_names_its_resource_metadata() {
        assert_eq!(
            resource_metadata(
                r#"Bearer error="invalid_token", resource_metadata="https://a.example/.well-known/oauth-protected-resource/mcp""#
            )
            .as_deref(),
            Some("https://a.example/.well-known/oauth-protected-resource/mcp")
        );
        assert_eq!(
            resource_metadata("Bearer resource_metadata=https://a.example/m, scope=x").as_deref(),
            Some("https://a.example/m")
        );
        assert_eq!(resource_metadata("Bearer realm=x"), None);
    }

    #[test]
    fn well_known_addresses_go_before_the_path() {
        let url = Url::parse("https://a.example/tenant/mcp/").unwrap();
        assert_eq!(
            well_known(&url, "oauth-protected-resource"),
            [
                "https://a.example/.well-known/oauth-protected-resource/tenant/mcp",
                "https://a.example/.well-known/oauth-protected-resource",
            ]
        );
    }
}
