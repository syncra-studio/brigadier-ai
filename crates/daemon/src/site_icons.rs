//! Web sites' icons for the links in the thread's answers, fetched from each site itself (no
//! third-party icon service, so no other party learns which pages an answer linked) and kept
//! in a small cache in the data folder.
//!
//! Only HTTPS sites by name are asked (never an address, `localhost` or a name without a dot), with
//! no cookies, no referrer and short timeouts. The same holds for every connection on the way, an
//! icon link's or a redirect's, and each name must resolve to public addresses only, so no page
//! can point the daemon at this machine or the local network. A site with no icon, or one that
//! can't be reached, is remembered for a week, so a thread's renders never ask it again meanwhile.

use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;
use ureq::config::Config;
use ureq::http::Uri;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

/// The cache stays under this many bytes: the oldest icons go first.
const CACHE_BYTES: u64 = 5 * 1024 * 1024;
/// An icon larger than this isn't kept.
const ICON_BYTES: u64 = 100 * 1024;
/// Of a site's page, only this much is read looking for its icon link.
const PAGE_BYTES: u64 = 256 * 1024;
/// How long a site without an icon isn't asked again.
const MISS_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Each fetch's whole time, its redirects' too.
const TIMEOUT: Duration = Duration::from_secs(3);
/// Icons looked up in this run, kept in memory; past it the disk cache answers.
const REMEMBERED: usize = 512;

/// The icons: the cache folder, and each site's lookup in this run (shared while it runs).
pub struct SiteIcons {
    dir: PathBuf,
    lookups: Mutex<HashMap<String, Arc<OnceCell<Option<String>>>>>,
}

impl SiteIcons {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            lookups: Mutex::new(HashMap::new()),
        }
    }

    /// The icon of `origin` (`https://host`) as a `data:` URL; `None` when the site isn't one to
    /// ask, has no icon, or can't be reached.
    pub async fn icon(&self, origin: &str) -> Option<String> {
        let host = host_of(origin)?;
        let cell = {
            let mut lookups = self
                .lookups
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if lookups.len() >= REMEMBERED && !lookups.contains_key(&host) {
                lookups.clear();
            }
            lookups.entry(host.clone()).or_default().clone()
        };
        let dir = self.dir.clone();
        cell.get_or_init(|| async move {
            tokio::task::spawn_blocking(move || lookup(&dir, &host))
                .await
                .ok()
                .flatten()
        })
        .await
        .clone()
    }
}

/// The host of an HTTPS origin worth asking: a name with a dot, never an address or `localhost`.
fn host_of(origin: &str) -> Option<String> {
    let rest = origin.trim().strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?.to_ascii_lowercase();
    // A port or credentials make it no plain site.
    if host.is_empty() || host.contains(['@', ':', '[', ']']) || !host.contains('.') {
        return None;
    }
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return None;
    }
    // An address, not a name.
    if host
        .split('.')
        .all(|part| part.chars().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    if !host
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return None;
    }
    Some(host)
}

/// The cache's file for a host, and its marker for a miss.
fn paths(dir: &Path, host: &str) -> (PathBuf, PathBuf) {
    let key: String = Sha256::digest(host.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    (
        dir.join(format!("{key}.icon")),
        dir.join(format!("{key}.miss")),
    )
}

/// From the cache, else from the site (then kept, or its miss remembered).
fn lookup(dir: &Path, host: &str) -> Option<String> {
    let (icon, miss) = paths(dir, host);
    if let Ok(text) = std::fs::read_to_string(&icon) {
        return Some(text);
    }
    let missed = std::fs::metadata(&miss)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.elapsed().ok())
        .is_some_and(|age| age < MISS_FOR);
    if missed {
        return None;
    }
    let found = fetch_icon(host);
    let _ = std::fs::create_dir_all(dir);
    match &found {
        Some(url) => {
            let _ = std::fs::write(&icon, url);
            let _ = std::fs::remove_file(&miss);
            trim(dir);
        }
        None => {
            let _ = std::fs::write(&miss, b"");
        }
    }
    found
}

/// Keeps the cache under `CACHE_BYTES`, the oldest files first.
fn trim(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            meta.is_file().then(|| {
                (
                    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    meta.len(),
                    entry.path(),
                )
            })
        })
        .collect();
    let mut total: u64 = files.iter().map(|(_, len, _)| len).sum();
    files.sort_by_key(|(at, _, _)| *at);
    for (_, len, path) in files {
        if total <= CACHE_BYTES {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .https_only(true)
        // Followed by `fetch`, which checks each hop first.
        .max_redirects(0)
        .http_status_as_error(false)
        .build();
    ureq::Agent::with_parts(config, DefaultConnector::default(), PublicSites::default())
}

/// The agent's name lookups: every site it connects to directly goes only to a site by name
/// (`host_of`) whose addresses are all public. Checked at connect time, so a name that changes its
/// address between two lookups can't slip through either. A proxy the user configured
/// (`HTTPS_PROXY`) is theirs to reach, wherever it is; through it the proxy resolves the site, and
/// `fetch` has checked its name.
#[derive(Debug, Default)]
struct PublicSites(DefaultResolver);

impl Resolver for PublicSites {
    fn resolve(
        &self,
        uri: &Uri,
        config: &Config,
        timeout: NextTimeout,
    ) -> Result<ResolvedSocketAddrs, ureq::Error> {
        if config.proxy().is_some_and(|proxy| proxy.uri() == uri) {
            return self.0.resolve(uri, config, timeout);
        }
        if !named(uri) {
            return Err(ureq::Error::HostNotFound);
        }
        let addresses = self.0.resolve(uri, config, timeout)?;
        if addresses.iter().all(|address| public(address.ip())) {
            Ok(addresses)
        } else {
            Err(ureq::Error::HostNotFound)
        }
    }
}

/// An HTTPS address of a site by name, as `host_of` allows.
fn named(uri: &Uri) -> bool {
    uri.scheme_str() == Some("https")
        && uri
            .authority()
            .is_some_and(|authority| host_of(&format!("https://{authority}")).is_some())
}

/// An address on the internet at large: not this machine, the local network, a carrier's shared
/// range, a link's own or a multicast group's.
fn public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

/// The site's `/favicon.ico`, else the icon its home page links to.
fn fetch_icon(host: &str) -> Option<String> {
    let agent = agent();
    if let Some(url) = fetch_image(&agent, &format!("https://{host}/favicon.ico")) {
        return Some(url);
    }
    let (page, _, at) = fetch(&agent, &format!("https://{host}/"), PAGE_BYTES)?;
    let href = icon_link(&String::from_utf8_lossy(&page))?;
    fetch_image(&agent, &resolve(&at, &href)?)
}

/// Redirects a fetch follows.
const REDIRECTS: usize = 3;

/// A GET's body, its content type and the address that answered (after redirects), when it
/// answered 200 within `limit` bytes and `TIMEOUT`. Each address on the way, the first and every
/// redirect's, must be a site by name.
fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Option<(Vec<u8>, String, String)> {
    let deadline = Instant::now() + TIMEOUT;
    let mut url = url.to_owned();
    let mut hops = 0;
    let response = loop {
        let uri: Uri = url.parse().ok()?;
        if !named(&uri) {
            return None;
        }
        // What is left of the fetch's time, for this hop and its body.
        let left = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())?;
        let response = agent
            .get(&url)
            .config()
            .timeout_global(Some(left))
            .build()
            .call()
            .ok()?;
        if !response.status().is_redirection() {
            break response;
        }
        hops += 1;
        let location = response.headers().get("location")?.to_str().ok()?;
        if hops > REDIRECTS {
            return None;
        }
        url = resolve(&url, location)?;
    };
    if response.status().as_u16() != 200 {
        return None;
    }
    let kind = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some((bytes, kind, url))
}

/// An image at `url` as a `data:` URL, when it is one.
fn fetch_image(agent: &ureq::Agent, url: &str) -> Option<String> {
    let (bytes, kind, _) = fetch(agent, url, ICON_BYTES)?;
    let mime = image_kind(&bytes, &kind)?;
    Some(format!("data:{mime};base64,{}", BASE64.encode(&bytes)))
}

/// The image type of `bytes`, by their first bytes (servers often send icons as anything).
fn image_kind(bytes: &[u8], declared: &str) -> Option<&'static str> {
    if bytes.starts_with(&[0, 0, 1, 0]) {
        return Some("image/x-icon");
    }
    if bytes.starts_with(b"\x89PNG") {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    if declared == "image/svg+xml"
        || head.trim_start().starts_with("<svg")
        || (head.contains("<?xml") && head.contains("<svg"))
    {
        return Some("image/svg+xml");
    }
    None
}

/// The `href` of the page's icon link: `rel="icon"`, `"shortcut icon"` or `"apple-touch-icon"`.
fn icon_link(page: &str) -> Option<String> {
    let lower = page.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find("<link") {
        let start = from + at;
        let end = start + lower[start..].find('>')?;
        let tag = &page[start..end];
        let rel = attribute(tag, "rel")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if rel
            .split_whitespace()
            .any(|word| word == "icon" || word == "apple-touch-icon")
            && let Some(href) = attribute(tag, "href")
        {
            return Some(href);
        }
        from = end;
    }
    None
}

/// An attribute's value in a tag, quoted or not.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut from = 0;
    while let Some(at) = lower[from..].find(name) {
        let start = from + at;
        from = start + name.len();
        // A whole attribute name, followed by `=`.
        let before = lower[..start].chars().last();
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        let rest = lower[from..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let offset = tag.len() - rest.len();
        let value = tag[offset..].trim_start();
        let found = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => value[1..].split(quote).next(),
            _ => value.split(|c: char| c.is_whitespace() || c == '>').next(),
        };
        return found.map(|text| text.replace("&amp;", "&"));
    }
    None
}

/// An icon link or a redirect's location resolved against the address it came from (`base`, an
/// HTTPS address): absolute HTTPS, protocol-relative, a path from the site's root, or one from
/// `base`'s folder.
fn resolve(base: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("data:"))
    {
        return None;
    }
    // A scheme in any case (`HTTPS://`), written as `https://`.
    if href
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
    {
        return Some(format!("https://{}", &href[8..]));
    }
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if href.contains("://") {
        return None;
    }
    let base: Uri = base.parse().ok()?;
    let host = base.host()?;
    if href.starts_with('/') {
        return Some(format!("https://{host}{href}"));
    }
    let path = base.path();
    let folder = &path[..path.rfind('/').map_or(0, |at| at + 1)];
    let folder = if folder.is_empty() { "/" } else { folder };
    // Its `.` and `..` resolved, as a browser does (servers needn't).
    let (relative, rest) = href.split_at(href.find(['?', '#']).unwrap_or(href.len()));
    let joined = format!("{folder}{relative}");
    let mut kept: Vec<&str> = Vec::new();
    let segments: Vec<&str> = joined.split('/').skip(1).collect();
    for (index, segment) in segments.iter().enumerate() {
        match *segment {
            "." => {}
            ".." => {
                kept.pop();
            }
            other => kept.push(other),
        }
        // A trailing `.` or `..` still names a folder.
        if index + 1 == segments.len() && (*segment == "." || *segment == "..") {
            kept.push("");
        }
    }
    Some(format!("https://{host}/{}{rest}", kept.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_sites_by_name_are_asked() {
        assert_eq!(host_of("https://nodejs.org").as_deref(), Some("nodejs.org"));
        assert_eq!(
            host_of("https://Docs.Rust-Lang.org/std/").as_deref(),
            Some("docs.rust-lang.org")
        );
        for origin in [
            "http://nodejs.org",
            "https://localhost",
            "https://127.0.0.1",
            "https://192.168.1.1",
            "https://[::1]",
            "https://intranet",
            "https://printer.local",
            "https://user@example.com",
            "https://example.com:8443",
            "file:///etc/passwd",
        ] {
            assert_eq!(host_of(origin), None, "{origin}");
        }
    }

    #[test]
    fn a_page_s_icon_link_is_found_and_resolved() {
        let page = r#"<html><head><link rel="stylesheet" href="/a.css"><LINK REL="shortcut icon" HREF='/static/fav.png?v=2&amp;x=1'></head>"#;
        let href = icon_link(page).unwrap();
        assert_eq!(href, "/static/fav.png?v=2&x=1");
        assert_eq!(
            resolve("https://nodejs.org/", &href).as_deref(),
            Some("https://nodejs.org/static/fav.png?v=2&x=1")
        );
        assert_eq!(
            icon_link(r#"<link rel=apple-touch-icon href=touch.png>"#).as_deref(),
            Some("touch.png")
        );
        assert_eq!(
            resolve("https://a.dev/", "touch.png").as_deref(),
            Some("https://a.dev/touch.png")
        );
        assert_eq!(
            resolve("https://a.dev/", "//cdn.a.dev/i.ico").as_deref(),
            Some("https://cdn.a.dev/i.ico")
        );
        // An HTTP icon isn't fetched.
        assert_eq!(resolve("https://a.dev/", "http://a.dev/i.ico"), None);
        assert_eq!(
            resolve("https://a.dev/", "HTTPS://cdn.a.dev/i.png").as_deref(),
            Some("https://cdn.a.dev/i.png")
        );
        assert_eq!(resolve("https://a.dev/", "HTTP://a.dev/i.ico"), None);
        // A page or a redirect deeper in the site: a bare name is in its folder.
        assert_eq!(
            resolve("https://a.dev/assets/old.ico?v=1", "new.ico").as_deref(),
            Some("https://a.dev/assets/new.ico")
        );
        assert_eq!(
            resolve("https://a.dev/en/", "./i.png").as_deref(),
            Some("https://a.dev/en/i.png")
        );
        assert_eq!(
            resolve("https://a.dev/en/home", "/i.png").as_deref(),
            Some("https://a.dev/i.png")
        );
        assert_eq!(
            resolve("https://a.dev/assets/old.ico", "../new.ico?v=../2").as_deref(),
            Some("https://a.dev/new.ico?v=../2")
        );
        assert_eq!(
            resolve("https://a.dev/a/b/c.ico", "../../../../x/./y.ico").as_deref(),
            Some("https://a.dev/x/y.ico")
        );
        assert_eq!(
            resolve("https://a.dev/a/b/", "..").as_deref(),
            Some("https://a.dev/a/")
        );
        assert_eq!(icon_link(r#"<link rel="preload" href="/x">"#), None);
    }

    #[test]
    fn only_public_addresses_of_sites_by_name_are_connected_to() {
        for address in ["93.184.216.34", "2606:2800:220:1::1", "1.1.1.1"] {
            assert!(public(address.parse().unwrap()), "{address}");
        }
        for address in [
            "127.0.0.1",
            "10.0.0.8",
            "172.16.4.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:192.168.0.2",
        ] {
            assert!(!public(address.parse().unwrap()), "{address}");
        }
        // An icon link or a redirect to an address, `localhost` or a port is refused before any
        // lookup, as a link in an answer is.
        assert!(named(
            &"https://nodejs.org/static/favicon.png".parse().unwrap()
        ));
        for url in [
            "https://127.0.0.1/favicon.ico",
            "https://localhost/favicon.ico",
            "https://example.com:8443/favicon.ico",
            "https://[::1]/favicon.ico",
            "https://router.local/favicon.ico",
            "http://example.com/favicon.ico",
        ] {
            assert!(!named(&url.parse().unwrap()), "{url}");
        }
        // The user's own proxy is reached wherever it is.
        let config = Config::builder()
            .proxy(Some(ureq::Proxy::new("http://127.0.0.1:9").unwrap()))
            .build();
        let proxy = config.proxy().unwrap().uri().clone();
        let resolved = PublicSites::default()
            .resolve(
                &proxy,
                &config,
                NextTimeout {
                    after: ureq::unversioned::transport::time::Duration::NotHappening,
                    reason: ureq::Timeout::Resolve,
                },
            )
            .unwrap();
        assert!(resolved.iter().all(|address| address.ip().is_loopback()));
    }

    /// Over the network: `cargo test -p brigadier-daemon site_icons -- --ignored`.
    #[test]
    #[ignore]
    fn a_real_site_s_icon_comes_through_the_checks() {
        assert!(fetch_icon("nodejs.org").is_some_and(|url| url.starts_with("data:image/")));
        assert!(fetch_icon("www.rust-lang.org").is_some());
        // Its /favicon.ico redirects to www., a hop `fetch` checks and follows.
        assert!(fetch_icon("python.org").is_some());
    }

    #[test]
    fn icons_are_told_by_their_bytes() {
        assert_eq!(image_kind(&[0, 0, 1, 0, 1], ""), Some("image/x-icon"));
        assert_eq!(image_kind(b"\x89PNG\r\n", "text/plain"), Some("image/png"));
        assert_eq!(image_kind(b"<svg xmlns=\"\">", ""), Some("image/svg+xml"));
        assert_eq!(image_kind(b"<!doctype html>", "text/html"), None);
    }

    #[test]
    fn the_cache_keeps_icons_and_remembers_misses() {
        let dir =
            std::env::temp_dir().join(format!("brigadier-site-icons-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (icon, miss) = paths(&dir, "cached.example");
        std::fs::write(&icon, "data:image/png;base64,AAAA").unwrap();
        // Kept: answered from the cache, without the network.
        assert_eq!(
            lookup(&dir, "cached.example").as_deref(),
            Some("data:image/png;base64,AAAA")
        );
        // A recent miss: not asked again.
        let (_, missed) = paths(&dir, "missed.example");
        std::fs::write(&missed, b"").unwrap();
        assert_eq!(lookup(&dir, "missed.example"), None);
        let _ = miss;
        // Over the cap, the oldest go.
        for index in 0..3 {
            std::fs::write(
                dir.join(format!("big-{index}.icon")),
                vec![b'x'; 2 * 1024 * 1024],
            )
            .unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        trim(&dir);
        let left: u64 = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.metadata().unwrap().len())
            .sum();
        assert!(left <= CACHE_BYTES, "{left}");
        assert!(dir.join("big-2.icon").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
