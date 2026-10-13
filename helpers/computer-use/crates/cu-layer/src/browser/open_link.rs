//! Opens links in a web browser that is already running, in a new window,
//! without bringing the browser to the front.
//!
//! Handing links to a running browser through LaunchServices makes it come to
//! the front, interrupting the user. Instead the browser is asked directly:
//! over its debugging port when it has one (a background window, no
//! activation at all), else by Apple Events when the user has already
//! allowed them (this never shows macOS's Automation prompt). When neither
//! works the caller keeps using LaunchServices.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use super::browser_js::{escape_for_applescript_string, BrowserJs};
use super::cdp_client::{listening_ports, CdpClient};
use platform_macos::windows::WindowInfo;

/// How long to wait for the new window to show up in the window list.
const NEW_WINDOW_WAIT: Duration = Duration::from_millis(1500);

/// The browser families whose Apple Event dictionaries differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserKind {
    Chromium,
    Safari,
}

/// How the links reached the browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenedBy {
    /// The browser's debugging port.
    DevTools(u16),
    AppleEvents,
}

#[derive(Clone, Copy, Debug)]
pub struct OpenedLinks {
    pub pid: i32,
    pub by: OpenedBy,
}

/// The browser family of `bundle_id`, or `None` when it isn't a web browser
/// this knows how to open a window in.
pub fn browser_kind(bundle_id: &str) -> Option<BrowserKind> {
    const CHROMIUM: &[&str] = &[
        "com.google.Chrome",
        "com.google.Chrome.beta",
        "com.google.Chrome.dev",
        "com.google.Chrome.canary",
        "com.google.chrome.for.testing",
        "org.chromium.Chromium",
        "com.brave.Browser",
        "com.brave.Browser.beta",
        "com.brave.Browser.nightly",
        "com.microsoft.edgemac",
        "com.microsoft.edgemac.Beta",
        "com.microsoft.edgemac.Dev",
        "com.microsoft.edgemac.Canary",
        "com.vivaldi.Vivaldi",
        "company.thebrowser.Browser",
    ];
    const SAFARI: &[&str] = &["com.apple.Safari", "com.apple.SafariTechnologyPreview"];
    if CHROMIUM.contains(&bundle_id) {
        Some(BrowserKind::Chromium)
    } else if SAFARI.contains(&bundle_id) {
        Some(BrowserKind::Safari)
    } else {
        None
    }
}

/// A link a browser window can show: a web address (`http` or `https`) or a
/// local page (`file://`), with no spaces or control characters.
pub fn is_web_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("file:///"))
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// `Target.createTarget` parameters that open `url` in a new window the
/// browser does not bring to the front.
pub fn background_window_params(url: &str) -> serde_json::Value {
    serde_json::json!({ "url": url, "newWindow": true, "background": true })
}

/// An Apple Event script that opens each link in a new window of the
/// browser, never activating it.
pub fn new_window_script(kind: BrowserKind, bundle_id: &str, urls: &[String]) -> String {
    let mut script = format!(
        "tell application id \"{}\"\n",
        escape_for_applescript_string(bundle_id)
    );
    for url in urls {
        let url = escape_for_applescript_string(url);
        match kind {
            BrowserKind::Chromium => script.push_str(&format!(
                "set newWindow to make new window\nset URL of active tab of newWindow to \"{url}\"\n"
            )),
            BrowserKind::Safari => script.push_str(&format!(
                "make new document with properties {{URL:\"{url}\"}}\n"
            )),
        }
    }
    script.push_str("end tell\nreturn \"opened\"");
    script
}

/// Puts windows that are not in `before` first, and among them the ones big
/// enough to show a page before the browser's own small helper windows (a
/// toolbar or address-bar popup opened along with a new window), keeping the
/// order within each group. Returns how many new windows can show a page.
pub fn new_windows_first(windows: &mut [WindowInfo], before: &HashSet<u32>) -> usize {
    windows.sort_by_key(|w| (before.contains(&w.window_id), !shows_a_page(w)));
    windows
        .iter()
        .take_while(|w| !before.contains(&w.window_id) && shows_a_page(w))
        .count()
}

/// A window big enough to show a page, unlike the browser's thin toolbar,
/// address-bar popup and placeholder windows.
fn shows_a_page(window: &WindowInfo) -> bool {
    const MIN_SIDE: f64 = 200.0;
    window.bounds.width >= MIN_SIDE && window.bounds.height >= MIN_SIDE
}

/// The pid's windows with the ones opened since `before` first, waiting a
/// little for `expected` new windows to appear. Returns how many are new.
pub fn wait_for_new_windows(
    pid: i32,
    before: &HashSet<u32>,
    expected: usize,
) -> (Vec<WindowInfo>, usize) {
    let deadline = Instant::now() + NEW_WINDOW_WAIT;
    loop {
        let mut windows = current_windows_for_pid(pid);
        let new = new_windows_first(&mut windows, before);
        if new >= expected || Instant::now() >= deadline {
            return (windows, new);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Layer-0 windows of `pid` with a real size.
pub fn current_windows_for_pid(pid: i32) -> Vec<WindowInfo> {
    platform_macos::windows::all_windows()
        .into_iter()
        .filter(|w| w.pid == pid && w.layer == 0)
        .filter(|w| w.bounds.width > 1.0 && w.bounds.height > 1.0)
        .collect()
}

/// Opens `urls` in a new window of the running browser the caller named,
/// without bringing it to the front. `None` when the app is not a running
/// web browser, a link is not a web address or local page, or the browser could not be
/// asked directly; the caller then opens the links the usual way.
pub async fn open_in_running_browser(
    bundle_id: Option<&str>,
    name: Option<&str>,
    urls: &[String],
    requested_port: Option<u16>,
) -> Option<OpenedLinks> {
    if urls.is_empty() || !urls.iter().all(|u| is_web_link(u)) {
        return None;
    }
    let app = platform_macos::apps::list_running_apps()
        .into_iter()
        .find(|a| match bundle_id {
            Some(bid) => a.bundle_id.as_deref() == Some(bid),
            None => name == Some(a.name.as_str()),
        })?;
    let app_bundle = app.bundle_id.clone()?;
    let kind = browser_kind(&app_bundle)?;
    let pid = app.pid;
    // Catch the new window as it opens, also when it lands on another Space.
    crate::offspace::watch(pid);

    if let Some(port) = debugging_port(pid, requested_port).await {
        let params = urls.iter().map(|u| background_window_params(u)).collect();
        match CdpClient::call_on_browser(port, "Target.createTarget", params).await {
            Ok(_) => {
                return Some(OpenedLinks {
                    pid,
                    by: OpenedBy::DevTools(port),
                })
            }
            Err(error) => tracing::info!(
                target: "cu_layer::browser::open_link",
                pid,
                port,
                %error,
                "opening links over the debugging port failed; trying Apple Events"
            ),
        }
    }

    let script = new_window_script(kind, &app_bundle, urls);
    match BrowserJs::run_script_if_allowed(&app_bundle, script).await {
        Ok(_) => Some(OpenedLinks {
            pid,
            by: OpenedBy::AppleEvents,
        }),
        Err(error) => {
            tracing::info!(
                target: "cu_layer::browser::open_link",
                pid,
                %error,
                "opening links by Apple Events failed; using LaunchServices"
            );
            None
        }
    }
}

/// The browser's debugging port: the one asked for when the browser itself
/// listens on it, else one it listens on that answers as a debugging port,
/// also when the browser has no page open.
async fn debugging_port(pid: i32, requested: Option<u16>) -> Option<u16> {
    let mut ports = listening_ports(pid).await;
    if let Some(port) = requested.filter(|p| ports.contains(p)) {
        ports.retain(|p| *p != port);
        ports.insert(0, port);
    }
    for port in ports {
        if CdpClient::browser_websocket_url(port).await.is_ok() {
            return Some(port);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_are_told_apart_from_other_apps() {
        for bid in [
            "com.google.Chrome",
            "com.google.Chrome.canary",
            "com.google.chrome.for.testing",
            "com.brave.Browser",
            "com.microsoft.edgemac",
            "com.vivaldi.Vivaldi",
            "company.thebrowser.Browser",
        ] {
            assert_eq!(browser_kind(bid), Some(BrowserKind::Chromium), "{bid}");
        }
        assert_eq!(browser_kind("com.apple.Safari"), Some(BrowserKind::Safari));
        // Electron apps embed Chromium but are not browsers.
        assert_eq!(browser_kind("com.microsoft.VSCode"), None);
        assert_eq!(browser_kind("com.apple.finder"), None);
        // A Chrome app shim is its own app, not the browser.
        assert_eq!(browser_kind("com.google.Chrome.app.abcdef"), None);
    }

    #[test]
    fn only_web_addresses_and_local_pages_go_to_a_browser_window() {
        assert!(is_web_link("https://example.com/a?b=c#d"));
        assert!(is_web_link("HTTP://example.com"));
        assert!(is_web_link("file:///tmp/page.html"));
        assert!(!is_web_link("file://host/tmp/page.html"));
        assert!(!is_web_link("/tmp/page.html"));
        assert!(!is_web_link("mailto:someone@example.com"));
        assert!(!is_web_link("https://example.com/a b"));
        assert!(!is_web_link("https://example.com/\n"));
    }

    #[test]
    fn the_new_target_opens_in_a_background_window() {
        assert_eq!(
            background_window_params("https://example.com"),
            serde_json::json!({
                "url": "https://example.com",
                "newWindow": true,
                "background": true
            })
        );
    }

    #[test]
    fn chromium_script_opens_each_link_in_a_new_window_without_activating() {
        let script = new_window_script(
            BrowserKind::Chromium,
            "com.google.Chrome",
            &["https://a.example".into(), "https://b.example/\"x\"".into()],
        );
        assert_eq!(
            script,
            "tell application id \"com.google.Chrome\"\n\
             set newWindow to make new window\n\
             set URL of active tab of newWindow to \"https://a.example\"\n\
             set newWindow to make new window\n\
             set URL of active tab of newWindow to \"https://b.example/\\\"x\\\"\"\n\
             end tell\n\
             return \"opened\""
        );
        assert!(!script.contains("activate"));
    }

    #[test]
    fn safari_script_makes_a_new_document_with_the_link() {
        let script = new_window_script(
            BrowserKind::Safari,
            "com.apple.Safari",
            &["https://a.example".into()],
        );
        assert_eq!(
            script,
            "tell application id \"com.apple.Safari\"\n\
             make new document with properties {URL:\"https://a.example\"}\n\
             end tell\n\
             return \"opened\""
        );
        assert!(!script.contains("activate"));
    }

    fn window(window_id: u32) -> WindowInfo {
        sized_window(window_id, 800.0, 600.0)
    }

    fn sized_window(window_id: u32, width: f64, height: f64) -> WindowInfo {
        serde_json::from_value(serde_json::json!({
            "window_id": window_id,
            "pid": 1,
            "app_name": "Browser",
            "title": "",
            "bounds": { "x": 0.0, "y": 0.0, "width": width, "height": height },
            "layer": 0,
            "z_index": 0,
            "is_on_screen": true,
            "on_current_space": null,
            "space_ids": null,
        }))
        .unwrap()
    }

    #[test]
    fn windows_opened_since_the_snapshot_are_listed_first() {
        let before: HashSet<u32> = [10, 20].into();
        let mut windows = vec![window(10), window(31), window(20), window(32)];
        assert_eq!(new_windows_first(&mut windows, &before), 2);
        let ids: Vec<u32> = windows.iter().map(|w| w.window_id).collect();
        assert_eq!(ids, [31, 32, 10, 20]);
    }

    #[test]
    fn a_new_page_window_comes_before_the_browsers_new_helper_windows() {
        let before: HashSet<u32> = [10].into();
        let mut windows = vec![
            window(10),
            sized_window(41, 1038.0, 88.0),
            sized_window(42, 1.0, 1.0),
            window(40),
        ];
        assert_eq!(new_windows_first(&mut windows, &before), 1);
        let ids: Vec<u32> = windows.iter().map(|w| w.window_id).collect();
        assert_eq!(ids, [40, 41, 42, 10]);
    }

    #[test]
    fn no_new_window_leaves_the_order_alone() {
        let before: HashSet<u32> = [10, 20].into();
        let mut windows = vec![window(20), window(10)];
        assert_eq!(new_windows_first(&mut windows, &before), 0);
        let ids: Vec<u32> = windows.iter().map(|w| w.window_id).collect();
        assert_eq!(ids, [20, 10]);
    }
}
