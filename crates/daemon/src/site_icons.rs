//! Web sites' icons for the links in the thread's answers, fetched from each site itself (no
//! third-party icon service, so no other party learns which pages an answer linked) and kept
//! in a small cache in the data folder.
//!
//! Only HTTPS sites by name are asked (never an address, `localhost` or a name without a dot), with
//! no cookies, no referrer and short timeouts. A site with no icon, or one that can't be reached,
//! is remembered for a week, so a thread's renders never ask it again meanwhile.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};
use tokio::sync::OnceCell;

/// The cache stays under this many bytes: the oldest icons go first.
const CACHE_BYTES: u64 = 5 * 1024 * 1024;
/// An icon larger than this isn't kept.
const ICON_BYTES: u64 = 100 * 1024;
/// Of a site's page, only this much is read looking for its icon link.
const PAGE_BYTES: u64 = 256 * 1024;
/// How long a site without an icon isn't asked again.
const MISS_FOR: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Each fetch's whole time.
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
    ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .https_only(true)
        .max_redirects(3)
        .http_status_as_error(false)
        .build()
        .into()
}

/// The site's `/favicon.ico`, else the icon its home page links to.
fn fetch_icon(host: &str) -> Option<String> {
    let agent = agent();
    if let Some(url) = fetch_image(&agent, &format!("https://{host}/favicon.ico")) {
        return Some(url);
    }
    let page = fetch(&agent, &format!("https://{host}/"), PAGE_BYTES)?;
    let href = icon_link(&String::from_utf8_lossy(&page.0))?;
    fetch_image(&agent, &resolve(host, &href)?)
}

/// A GET's body and its content type, when it answered 200 within `limit` bytes.
fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> Option<(Vec<u8>, String)> {
    let response = agent.get(url).call().ok()?;
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
    (bytes.len() as u64 <= limit).then_some((bytes, kind))
}

/// An image at `url` as a `data:` URL, when it is one.
fn fetch_image(agent: &ureq::Agent, url: &str) -> Option<String> {
    let (bytes, kind) = fetch(agent, url, ICON_BYTES)?;
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

/// An icon link resolved against the site: absolute HTTPS, protocol-relative, or a path.
fn resolve(host: &str, href: &str) -> Option<String> {
    let href = href.trim();
    if href.starts_with("data:") {
        return None;
    }
    if href.starts_with("https://") {
        return Some(href.to_owned());
    }
    if let Some(rest) = href.strip_prefix("//") {
        return Some(format!("https://{rest}"));
    }
    if href.contains("://") {
        return None;
    }
    let path = href.strip_prefix("./").unwrap_or(href);
    Some(if path.starts_with('/') {
        format!("https://{host}{path}")
    } else {
        format!("https://{host}/{path}")
    })
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
            resolve("nodejs.org", &href).as_deref(),
            Some("https://nodejs.org/static/fav.png?v=2&x=1")
        );
        assert_eq!(
            icon_link(r#"<link rel=apple-touch-icon href=touch.png>"#).as_deref(),
            Some("touch.png")
        );
        assert_eq!(
            resolve("a.dev", "touch.png").as_deref(),
            Some("https://a.dev/touch.png")
        );
        assert_eq!(
            resolve("a.dev", "//cdn.a.dev/i.ico").as_deref(),
            Some("https://cdn.a.dev/i.ico")
        );
        // An HTTP icon isn't fetched.
        assert_eq!(resolve("a.dev", "http://a.dev/i.ico"), None);
        assert_eq!(icon_link(r#"<link rel="preload" href="/x">"#), None);
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
