//! Keeps the model registry current (PLAN.md §6 Phase 5): a minute after launch and then daily,
//! and when the Usage page asks, the daemon fetches `registry/models.json` from the repository
//! with the ETag of the copy it has. The core checks what comes back and takes it only if it
//! reads within bounds and is newer ([`brigadier_core::routing::RegistryHolder`]).
//!
//! Only HTTPS from the repository. Development builds may point `BRIGADIER_REGISTRY_URL` at
//! another HTTPS address, or at plain HTTP on the loopback address (127.0.0.1 or ::1), to
//! verify updates against a local copy; release builds ignore it. The registry is not signed
//! yet: signing joins the app updater's key (Phase 10).

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use brigadier_core::routing::Fetched;
use tokio_util::sync::CancellationToken;

use crate::server::Daemon;

/// The published registry.
const REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/stephen-golban/brigadier-ai/main/registry/models.json";
/// The first check after launch, then how often.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Checks for a newer registry on schedule, until the daemon stops.
pub async fn keep_current(daemon: Arc<Daemon>, stop: CancellationToken) -> anyhow::Result<()> {
    let mut wait = FIRST_CHECK;
    loop {
        tokio::select! {
            () = stop.cancelled() => return Ok(()),
            () = tokio::time::sleep(wait) => {}
        }
        wait = CHECK_EVERY;
        check(&daemon).await;
    }
}

/// Asks the repository for a newer registry now and takes it if it passes the checks.
pub async fn check(daemon: &Daemon) {
    let holder = daemon.runtime.registry().clone();
    let etag = holder.etag();
    let result = tokio::task::spawn_blocking(move || {
        let fetched = match registry_url() {
            Ok(url) => fetch(&url, etag.as_deref()),
            Err(err) => Fetched::Failed(err),
        };
        holder.take(fetched)
    })
    .await;
    if matches!(result, Ok(true)) {
        daemon.runtime.rankings_changed().await;
    }
    if let Err(err) = result {
        tracing::warn!(error = %err, "the registry check stopped");
    }
}

/// Where the registry is fetched from.
fn registry_url() -> Result<String, String> {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("BRIGADIER_REGISTRY_URL") {
        let url = url.trim().to_owned();
        // By the parsed host, not a prefix: `http://127.0.0.1:80@example.com/` goes to
        // example.com.
        let allowed = url.parse::<ureq::http::Uri>().ok().is_some_and(|uri| {
            match (uri.scheme_str(), uri.host()) {
                (Some("https"), Some(_)) => true,
                (Some("http"), Some(host)) => {
                    host == "127.0.0.1" || host == "[::1]" || host == "::1"
                }
                _ => false,
            }
        });
        if allowed {
            return Ok(url);
        }
        return Err(format!(
            "BRIGADIER_REGISTRY_URL must be HTTPS, or HTTP on 127.0.0.1 or ::1 ({url})"
        ));
    }
    Ok(REGISTRY_URL.to_owned())
}

/// One conditional GET, capped at the registry's size limit.
fn fetch(url: &str, etag: Option<&str>) -> Fetched {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        // A redirect could leave HTTPS or the loopback address.
        .max_redirects(0)
        .build()
        .into();
    let mut request = agent.get(url);
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }
    let response = match request.call() {
        Ok(response) => response,
        Err(err) => return Fetched::Failed(format!("the repository couldn't be reached: {err}")),
    };
    match response.status().as_u16() {
        304 => return Fetched::NotModified,
        200 => {}
        status => return Fetched::Failed(format!("the repository answered {status}")),
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let limit = brigadier_core::routing::registry::MAX_BYTES as u64;
    let mut bytes = Vec::new();
    let read = response
        .into_body()
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes);
    if let Err(err) = read {
        return Fetched::Failed(format!("the download broke off: {err}"));
    }
    if bytes.len() as u64 > limit {
        return Fetched::Failed("the document is larger than a registry may be".into());
    }
    Fetched::Document {
        bytes,
        etag,
        source: url.to_owned(),
    }
}
