//! The proxy a worker reaches a reused sign-in's server through: HTTP on 127.0.0.1, one path
//! per worker and server (`/mcp/<id>`), which takes only that worker's key. A request without
//! the key is refused before its body is read. The proxy sends the request on with the
//! server's configured headers and a fresh access token, renewing it and replaying once after
//! a 401, and streams the answer back. Nothing it answers holds the worker's key or a token.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Weak};

use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{
    AUTHORIZATION, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap, HeaderName,
    HeaderValue, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
    WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

use super::McpAuth;

/// The largest request body passed on (MCP requests are JSON-RPC messages).
const BODY_MAX: usize = 16 * 1024 * 1024;

type Body = UnsyncBoxBody<Bytes, std::io::Error>;

/// Starts the proxy for `auth` on a free port of 127.0.0.1; it stops once `auth` is gone.
pub(super) async fn start(auth: Weak<McpAuth>) -> std::io::Result<SocketAddr> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let Some(auth) = auth.upgrade() else {
                break;
            };
            tokio::spawn(async move {
                let service = service_fn(move |request| {
                    let auth = auth.clone();
                    async move { Ok::<_, Infallible>(serve(&auth, request).await) }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    Ok(addr)
}

fn answer(status: StatusCode, text: &'static str) -> Response<Body> {
    let body = Full::new(Bytes::from_static(text.as_bytes()))
        .map_err(|never| match never {})
        .boxed_unsync();
    let mut response = Response::new(body);
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// Equal, compared in time that doesn't depend on where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

/// Headers that belong to one connection, not to the request or answer.
fn hop_by_hop(name: &HeaderName) -> bool {
    [
        CONNECTION,
        HOST,
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
        CONTENT_LENGTH,
    ]
    .contains(name)
        || name.as_str() == "keep-alive"
}

async fn serve(auth: &Arc<McpAuth>, request: Request<Incoming>) -> Response<Body> {
    let route = request
        .uri()
        .path()
        .strip_prefix("/mcp/")
        .and_then(|id| auth.route_of(id));
    let Some(route) = route else {
        return answer(StatusCode::NOT_FOUND, "No such server here.");
    };
    let key = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !key.is_some_and(|key| same(key, &route.grant)) || !(auth.alive)(&route.grant) {
        return answer(
            StatusCode::UNAUTHORIZED,
            "This address takes its worker's key only.",
        );
    }
    let (parts, body) = request.into_parts();
    let Ok(body) = Limited::new(body, BODY_MAX).collect().await else {
        return answer(
            StatusCode::PAYLOAD_TOO_LARGE,
            "The request couldn't be read.",
        );
    };
    let body = body.to_bytes();
    let mut headers = HeaderMap::new();
    for (name, value) in &parts.headers {
        if !hop_by_hop(name) && name != AUTHORIZATION {
            headers.append(name.clone(), value.clone());
        }
    }
    for (name, value) in &route.headers {
        if let (Ok(name), Ok(mut value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value.expose()),
        ) {
            value.set_sensitive(true);
            headers.insert(name, value);
        }
    }
    let mut token = match auth.token(&route, None).await {
        Ok(token) => token,
        Err(reason) => {
            tracing::warn!(owner = route.place.owner(), %reason, "an MCP request had no sign-in to go with it");
            return answer(
                StatusCode::UNAUTHORIZED,
                "No stored sign-in serves this server now.",
            );
        }
    };
    let mut replayed = false;
    loop {
        let mut sent = headers.clone();
        let Ok(mut bearer) = HeaderValue::from_str(&format!("Bearer {}", token.expose())) else {
            return answer(StatusCode::BAD_GATEWAY, "The stored sign-in can't be sent.");
        };
        bearer.set_sensitive(true);
        sent.insert(AUTHORIZATION, bearer);
        let upstream = auth
            .http
            .request(parts.method.clone(), &route.url)
            .headers(sent)
            .body(body.clone())
            .send()
            .await;
        let upstream = match upstream {
            Ok(upstream) => upstream,
            Err(err) => {
                tracing::warn!(error = %err.without_url(), "an MCP server couldn't be reached through the proxy");
                return answer(StatusCode::BAD_GATEWAY, "The server couldn't be reached.");
            }
        };
        if upstream.status() == StatusCode::UNAUTHORIZED {
            if replayed {
                return answer(
                    StatusCode::UNAUTHORIZED,
                    "The server refused the stored sign-in.",
                );
            }
            replayed = true;
            match auth.token(&route, Some(&token)).await {
                Ok(renewed) => {
                    token = renewed;
                    continue;
                }
                Err(reason) => {
                    tracing::warn!(owner = route.place.owner(), %reason, "an MCP sign-in the server refused wasn't renewed");
                    return answer(
                        StatusCode::UNAUTHORIZED,
                        "The server refused the stored sign-in.",
                    );
                }
            }
        }
        let mut response: Response<reqwest::Body> = upstream.into();
        // The CLI must not start a sign-in of its own against the proxy.
        response.headers_mut().remove(WWW_AUTHENTICATE);
        let names: Vec<HeaderName> = response
            .headers()
            .keys()
            .filter(|name| hop_by_hop(name))
            .cloned()
            .collect();
        for name in names {
            response.headers_mut().remove(name);
        }
        return response.map(|body| {
            body.map_err(|err| std::io::Error::other(err.without_url()))
                .boxed_unsync()
        });
    }
}
