//! MCP authorization: resource metadata -> issuer metadata -> our own public client.
use super::store::Grant;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, Url};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
pub(super) struct Metadata {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub registration_endpoint: Option<String>,
    pub revocation_endpoint: Option<String>,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
    #[serde(default)]
    pub client_id_metadata_document_supported: bool,
}
#[derive(Deserialize)]
struct ResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
    #[serde(default)]
    scopes_supported: Vec<String>,
}
pub(super) struct Discovery {
    pub metadata: Metadata,
    pub scopes: String,
}

pub(super) fn url(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "Invalid OAuth URL")?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("Invalid OAuth URL".into());
    }
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err("OAuth requires HTTPS".into());
    }
    Ok(url)
}

pub(super) fn random() -> String {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).expect("OS randomness unavailable");
    URL_SAFE_NO_PAD.encode(bytes)
}
pub(super) fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
pub(super) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parameter(header: &str, name: &str) -> Option<String> {
    // Bearer challenge parameters are comma-separated; quoted URL values contain no commas
    // unless escaped. URL parsing below rejects malformed challenges.
    header.split(',').find_map(|part| {
        let part = part.trim().strip_prefix("Bearer ").unwrap_or(part.trim());
        let (key, value) = part.split_once('=')?;
        (key.trim() == name).then(|| value.trim().trim_matches('"').to_owned())
    })
}

pub(super) async fn discover(client: &Client, resource: &str) -> Result<Discovery, String> {
    let resource_url = url(resource)?;
    let response = client
        .get(resource_url.clone())
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|_| "Cannot contact MCP resource")?;
    let challenge = response
        .headers()
        .get("www-authenticate")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let requested_scope = parameter(challenge, "scope");
    let mut candidates = Vec::new();
    if let Some(location) = parameter(challenge, "resource_metadata") {
        candidates.push(url(&location)?);
    }
    let origin = resource_url.origin().ascii_serialization();
    let path = resource_url.path().trim_end_matches('/');
    candidates.push(url(&format!(
        "{origin}/.well-known/oauth-protected-resource{path}"
    ))?);
    candidates.push(url(&format!(
        "{origin}/.well-known/oauth-protected-resource"
    ))?);
    let mut protected = None;
    for candidate in candidates {
        let Ok(response) = client
            .get(candidate)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
        else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        if let Ok(metadata) = response.json::<ResourceMetadata>().await {
            protected = Some(metadata);
            break;
        }
    }
    let protected = protected.ok_or("MCP protected-resource metadata unavailable")?;
    if url(&protected.resource)? != resource_url {
        return Err("MCP metadata resource mismatch".into());
    }
    let issuer = url(protected
        .authorization_servers
        .first()
        .ok_or("MCP metadata has no authorization server")?)?;
    let origin = issuer.origin().ascii_serialization();
    let path = issuer.path().trim_end_matches('/');
    let candidates = [
        format!("{origin}/.well-known/oauth-authorization-server{path}"),
        format!("{origin}/.well-known/openid-configuration{path}"),
        format!("{issuer}/.well-known/openid-configuration")
            .replace("//.well-known", "/.well-known"),
    ];
    for candidate in candidates {
        let Ok(response) = client
            .get(url(&candidate)?)
            .timeout(std::time::Duration::from_secs(20))
            .send()
            .await
        else {
            continue;
        };
        if !response.status().is_success() {
            continue;
        }
        let Ok(metadata) = response.json::<Metadata>().await else {
            continue;
        };
        if metadata.issuer != issuer.as_str().trim_end_matches('/')
            && metadata.issuer != issuer.as_str()
        {
            continue;
        }
        url(&metadata.authorization_endpoint)?;
        url(&metadata.token_endpoint)?;
        if let Some(endpoint) = &metadata.registration_endpoint {
            url(endpoint)?;
        }
        if let Some(endpoint) = &metadata.revocation_endpoint {
            url(endpoint)?;
        }
        if !metadata
            .code_challenge_methods_supported
            .iter()
            .any(|method| method == "S256")
        {
            return Err("Authorization server does not support PKCE S256".into());
        }
        return Ok(Discovery {
            metadata,
            scopes: requested_scope.unwrap_or_else(|| protected.scopes_supported.join(" ")),
        });
    }
    Err("OAuth authorization-server metadata unavailable".into())
}

pub(super) enum Registration {
    Client { id: String, secret: Option<String> },
    Refused,
}
pub(super) async fn register(
    client: &Client,
    metadata: &Metadata,
    redirect: &str,
    document: Option<&str>,
) -> Result<Registration, String> {
    if metadata.client_id_metadata_document_supported
        && let Some(document) = document
    {
        let document_url = url(document)?;
        if document_url.scheme() != "https" {
            return Err("Client metadata document requires HTTPS".into());
        }
        return Ok(Registration::Client {
            id: document.into(),
            secret: None,
        });
    }
    let Some(endpoint) = &metadata.registration_endpoint else {
        return Ok(Registration::Refused);
    };
    let response = client
        .post(url(endpoint)?)
        .timeout(std::time::Duration::from_secs(30))
        .json(&serde_json::json!({
            "client_name":"Brigadier", "redirect_uris":[redirect],
            "grant_types":["authorization_code", "refresh_token"], "response_types":["code"],
            "token_endpoint_auth_method":"none"
        }))
        .send()
        .await
        .map_err(|_| "OAuth client registration failed")?;
    if matches!(response.status().as_u16(), 400 | 401 | 403 | 405) {
        return Ok(Registration::Refused);
    }
    if !response.status().is_success() {
        return Err("OAuth client registration failed".into());
    }
    #[derive(Deserialize)]
    struct Registered {
        client_id: String,
        client_secret: Option<String>,
    }
    let registered: Registered = response
        .json()
        .await
        .map_err(|_| "Invalid OAuth client registration")?;
    if registered.client_id.is_empty() {
        return Err("Empty OAuth client ID".into());
    }
    Ok(Registration::Client {
        id: registered.client_id,
        secret: registered.client_secret,
    })
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    token_type: String,
}
pub(super) async fn exchange(
    client: &Client,
    resource: &str,
    mut grant: Grant,
    mut form: Vec<(&str, String)>,
) -> Result<Grant, String> {
    form.push(("client_id", grant.client_id.clone()));
    form.push(("resource", resource.into()));
    if let Some(secret) = &grant.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let response = client
        .post(url(&grant.token_endpoint)?)
        .timeout(std::time::Duration::from_secs(30))
        .form(&form)
        .send()
        .await
        .map_err(|_| "OAuth token request failed")?;
    if !response.status().is_success() {
        return Err("OAuth token request rejected; sign in again".into());
    }
    let token: TokenResponse = response
        .json()
        .await
        .map_err(|_| "Invalid OAuth token response")?;
    if token.access_token.is_empty() || !token.token_type.eq_ignore_ascii_case("bearer") {
        return Err("Invalid OAuth bearer token".into());
    }
    grant.access_token = token.access_token;
    if token.refresh_token.is_some() {
        grant.refresh_token = token.refresh_token;
    }
    grant.expires_at = token
        .expires_in
        .and_then(|seconds| now().checked_add(seconds))
        .unwrap_or(u64::MAX);
    Ok(grant)
}
