//! Opens links in a web browser that is already running, in a new window,
//! without bringing the browser to the front.
//!
//! Handing links to a running browser through LaunchServices makes it come to
//! the front, interrupting the user. Instead the browser is asked directly:
//! over its debugging port when it has one (a background window, no
//! activation at all), else by Apple Events when the user has already
//! allowed them (this never shows macOS's Automation prompt). When neither
//! works the caller keeps using LaunchServices.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use super::browser_js::{escape_for_applescript_string, BrowserJs};
use super::cdp_client::{listening_ports, CdpClient};
use platform_macos::windows::WindowInfo;

/// How long to wait for the new window to show up in the window list.
const NEW_WINDOW_WAIT: Duration = Duration::from_millis(1500);

/// How often the window list is looked at while waiting.
const NEW_WINDOW_POLL: Duration = Duration::from_millis(20);

/// How long to wait for a new page window to show its page's title instead
/// of the placeholder a new browser window starts with. The page's own
/// title usually follows within a few tens of milliseconds.
const PAGE_TITLE_WAIT: Duration = Duration::from_millis(250);

/// The smallest side of a window that shows a page, unlike the browser's
/// thin toolbar, address-bar popup and placeholder windows.
const MIN_PAGE_SIDE: f64 = 200.0;

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
    /// How many links at the end of the list could not be opened.
    pub unopened: usize,
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
    window.bounds.width >= MIN_PAGE_SIDE && window.bounds.height >= MIN_PAGE_SIDE
}

/// The pid's windows with the ones that show `urls` first: new windows, then
/// existing windows titled with a url's file or folder name (Finder reuses
/// its window, maybe one already showing it). Waits a little until each url
/// has such a window. Returns how
/// many are listed first. The wait watches the quick window list, so the
/// answer comes within a few tens of milliseconds of the window showing up.
pub fn wait_for_new_windows(
    pid: i32,
    titles_before: &HashMap<u32, String>,
    urls: &[String],
) -> (Vec<WindowInfo>, usize) {
    let before: HashSet<u32> = titles_before.keys().copied().collect();
    let names: Vec<String> = urls.iter().filter_map(|url| shown_name(url)).collect();
    let deadline = Instant::now() + NEW_WINDOW_WAIT;
    loop {
        let now = crate::window_list::windows(Some(pid));
        let done = count_new_page_windows(&now, &before) >= urls.len()
            || (names.len() == urls.len() && every_name_shown(&now, &names));
        if done || Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(NEW_WINDOW_POLL);
    }
    let mut windows = current_windows_for_pid(pid);
    let new = new_windows_first(&mut windows, &before);
    let retitled: HashSet<u32> = crate::window_list::windows(Some(pid))
        .into_iter()
        .filter(|w| before.contains(&w.id) && shows_a_name(w, &names))
        .map(|w| w.id)
        .collect();
    windows[new..].sort_by_key(|w| !retitled.contains(&w.window_id));
    let shown = windows[new..]
        .iter()
        .take_while(|w| retitled.contains(&w.window_id))
        .count();
    (windows, new + shown)
}

/// The names of the local files among `urls`, as their windows would be
/// titled. A link's page brings its own title, so links have none.
pub fn local_file_names(urls: &[String]) -> Vec<String> {
    urls.iter()
        .filter(|url| crate::launch::local_file_target(url).is_some())
        .filter_map(|url| shown_name(url))
        .collect()
}

/// Wait a little for a new process to show a window for each of `names`.
pub fn wait_for_titled_windows(pid: i32, names: &[String]) {
    let deadline = Instant::now() + NEW_WINDOW_WAIT;
    while !every_name_shown(&crate::window_list::windows(Some(pid)), names)
        && Instant::now() < deadline
    {
        std::thread::sleep(NEW_WINDOW_POLL);
    }
}

/// Lists the windows titled with one of `names` first, keeping the order
/// otherwise, and returns the names no window is titled with.
pub fn titled_windows_first(windows: &mut [WindowInfo], names: &[String]) -> Vec<String> {
    let titled =
        |title: &str, names: &[String]| names.iter().any(|name| title_shows_name(title, name));
    windows.sort_by_key(|window| !titled(&window.title, names));
    names
        .iter()
        .filter(|name| {
            !windows
                .iter()
                .any(|window| title_shows_name(&window.title, name))
        })
        .cloned()
        .collect()
}

/// The name a window showing `url` is titled with: its last path component.
/// shortcut: only %20 is decoded; other escaped names just wait out the cap.
fn shown_name(url: &str) -> Option<String> {
    let path = url
        .strip_prefix("file://")
        .unwrap_or(url)
        .trim_end_matches('/');
    let name = path.rsplit('/').next()?.replace("%20", " ");
    (!name.is_empty()).then_some(name)
}

/// Whether `window` is titled with one of `names` (apps may hide a file's
/// extension).
fn shows_a_name(window: &crate::window_list::Window, names: &[String]) -> bool {
    names
        .iter()
        .any(|name| title_shows_name(&window.title, name))
}

fn title_shows_name(title: &str, name: &str) -> bool {
    title == name
        || std::path::Path::new(name)
            .file_stem()
            .is_some_and(|stem| title == stem.to_string_lossy())
}

/// Whether every name is shown by one of `windows` that has its real size
/// (Finder titles a new window while it is still 0x0).
fn every_name_shown(windows: &[crate::window_list::Window], names: &[String]) -> bool {
    names.iter().all(|name| {
        windows.iter().any(|w| {
            w.width >= MIN_PAGE_SIDE
                && w.height >= MIN_PAGE_SIDE
                && shows_a_name(w, std::slice::from_ref(name))
        })
    })
}

/// [`wait_for_new_windows`] for links a browser opened: also waits a little
/// for each new page window to change from the title it first showed (a
/// placeholder such as "Untitled") to its page's own, so the answer names
/// the window the way the agent will look for it.
pub fn wait_for_new_pages(
    pid: i32,
    before: &HashSet<u32>,
    expected: usize,
) -> (Vec<WindowInfo>, usize) {
    let deadline = Instant::now() + NEW_WINDOW_WAIT;
    wait_for_page_windows(pid, before, expected, deadline);
    let first = new_page_titles(&crate::window_list::windows(Some(pid)), before);
    let titles_deadline = (Instant::now() + PAGE_TITLE_WAIT).min(deadline);
    while !first.is_empty() && Instant::now() < titles_deadline {
        let now = new_page_titles(&crate::window_list::windows(Some(pid)), before);
        if page_titles_settled(&first, &now) {
            break;
        }
        std::thread::sleep(NEW_WINDOW_POLL);
    }
    let mut windows = current_windows_for_pid(pid);
    let new = new_windows_first(&mut windows, before);
    (windows, new)
}

fn wait_for_page_windows(pid: i32, before: &HashSet<u32>, expected: usize, deadline: Instant) {
    while count_new_page_windows(&crate::window_list::windows(Some(pid)), before) < expected
        && Instant::now() < deadline
    {
        std::thread::sleep(NEW_WINDOW_POLL);
    }
}

/// The titles of the new windows big enough to show a page, by id.
fn new_page_titles(
    windows: &[crate::window_list::Window],
    before: &HashSet<u32>,
) -> HashMap<u32, String> {
    windows
        .iter()
        .filter(|w| {
            !before.contains(&w.id) && w.width >= MIN_PAGE_SIDE && w.height >= MIN_PAGE_SIDE
        })
        .map(|w| (w.id, w.title.clone()))
        .collect()
}

/// Whether every window in `first` now shows a title other than the one it
/// first showed.
fn page_titles_settled(first: &HashMap<u32, String>, now: &HashMap<u32, String>) -> bool {
    first.iter().all(|(id, was)| {
        now.get(id)
            .is_some_and(|title| !title.is_empty() && title != was)
    })
}

/// How many of `windows` are not in `before` and big enough to show a page.
fn count_new_page_windows(windows: &[crate::window_list::Window], before: &HashSet<u32>) -> usize {
    windows
        .iter()
        .filter(|w| {
            !before.contains(&w.id) && w.width >= MIN_PAGE_SIDE && w.height >= MIN_PAGE_SIDE
        })
        .count()
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
    let app = crate::launch::apps::running_app_named(bundle_id, name)?;
    let app_bundle = app.bundle_id.clone()?;
    let kind = browser_kind(&app_bundle)?;
    let pid = app.pid;
    // Catch the new window as it opens, also when it lands on another Space.
    crate::offspace::watch(pid);

    // Links the debugging port already opened are never opened again.
    let mut unopened = urls;
    let mut opened_over_port = None;
    if let Some(port) = debugging_port(pid, requested_port).await {
        let params = urls.iter().map(|u| background_window_params(u)).collect();
        let (opened, outcome) =
            CdpClient::call_on_browser(port, "Target.createTarget", params).await;
        match outcome {
            Ok(()) => {
                return Some(OpenedLinks {
                    pid,
                    by: OpenedBy::DevTools(port),
                    unopened: 0,
                })
            }
            Err(error) => tracing::info!(
                target: "cu_layer::browser::open_link",
                pid,
                port,
                opened = opened.len(),
                %error,
                "opening links over the debugging port failed; trying Apple Events"
            ),
        }
        if !opened.is_empty() {
            unopened = &urls[opened.len()..];
            opened_over_port = Some(port);
        }
    }

    let script = new_window_script(kind, &app_bundle, unopened);
    match BrowserJs::run_script_if_allowed(&app_bundle, script).await {
        Ok(_) => Some(OpenedLinks {
            pid,
            by: OpenedBy::AppleEvents,
            unopened: 0,
        }),
        Err(error) => {
            tracing::info!(
                target: "cu_layer::browser::open_link",
                pid,
                %error,
                "opening links by Apple Events failed; using LaunchServices"
            );
            // Some links are already open: report the rest as not opened
            // instead of opening every link again.
            opened_over_port.map(|port| OpenedLinks {
                pid,
                by: OpenedBy::DevTools(port),
                unopened: unopened.len(),
            })
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
    #[test]
    fn a_window_titled_with_the_folder_ends_the_wait() {
        use super::*;
        let window = |id, title: &str| crate::window_list::Window {
            id,
            pid: 1,
            width: 800.0,
            height: 600.0,
            title: title.into(),
        };
        let names = vec![shown_name("file:///tmp/my%20scratch/").unwrap()];
        assert_eq!(names, ["my scratch"]);
        assert!(!every_name_shown(&[window(7, "Downloads")], &names));
        assert!(every_name_shown(&[window(7, "my scratch")], &names));
        // A document title may hide its extension.
        let names = vec![shown_name("/tmp/doc.txt").unwrap()];
        assert!(every_name_shown(&[window(9, "doc")], &names));
        assert!(!every_name_shown(&[window(9, "doc.txt.bak")], &names));
        // A new window titled before it is sized doesn't end the wait yet.
        let sizing = crate::window_list::Window {
            width: 0.0,
            height: 0.0,
            ..window(10, "doc")
        };
        assert!(!every_name_shown(&[sizing], &names));
    }

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

    #[test]
    fn the_wait_counts_only_new_windows_that_show_a_page() {
        use crate::window_list::Window;
        let window = |id, width, height| Window {
            id,
            pid: 7,
            width,
            height,
            title: String::new(),
        };
        let before: HashSet<u32> = [1].into();
        let windows = [
            window(1, 900.0, 700.0),
            window(2, 900.0, 700.0),
            window(3, 900.0, 40.0),
            window(4, 200.0, 200.0),
        ];
        assert_eq!(count_new_page_windows(&windows, &before), 2);
    }
    #[test]
    fn a_new_page_window_is_awaited_until_it_shows_its_pages_title() {
        let titles = |rows: &[(u32, &str)]| -> HashMap<u32, String> {
            rows.iter().map(|(id, t)| (*id, t.to_string())).collect()
        };
        let first = titles(&[(2, "Untitled"), (3, "")]);
        assert!(!page_titles_settled(
            &first,
            &titles(&[(2, "cu fixture"), (3, "")])
        ));
        assert!(!page_titles_settled(
            &first,
            &titles(&[(2, "Untitled"), (3, "Docs")])
        ));
        assert!(page_titles_settled(
            &first,
            &titles(&[(2, "cu fixture"), (3, "Docs")])
        ));
        // A window that closed meanwhile is not settled; the wait is bounded.
        assert!(!page_titles_settled(&first, &titles(&[(2, "cu fixture")])));

        use crate::window_list::Window;
        let window = |id, height: f64, title: &str| Window {
            id,
            pid: 7,
            width: 900.0,
            height,
            title: title.to_owned(),
        };
        let before: HashSet<u32> = [1].into();
        assert_eq!(
            new_page_titles(
                &[
                    window(1, 700.0, "Old"),
                    window(2, 700.0, "Untitled"),
                    window(4, 40.0, "")
                ],
                &before
            ),
            titles(&[(2, "Untitled")])
        );
    }
}
