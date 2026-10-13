//! The side panel's Browser tab: a system webview laid over the tab's area of the window.
//!
//! It is a plain `wry` webview, not a Tauri one: Tauri gives each of its webviews the app's IPC
//! bridge, init scripts and custom protocols, and a web page must get none of that. This one
//! has no app IPC bridge or custom protocols. Its metadata-only handler accepts a bounded PNG
//! favicon for this tab, with no commands or access to the app. `NO_CAPTURE` takes screen
//! capture and speech recognition away (and on Windows the camera and microphone). Every tab keeps its cookies and site data in one
//! saved store of its own, apart from the app's own webview and kept per Brigadier data folder
//! (macOS: a WebKit data store named from the folder; Windows: a WebView2 folder in it), so
//! sign-ins last like in any browser. It opens only web pages, sends popups to the system
//! browser, and on macOS gives a page the camera or microphone only as the user chose for its
//! site (Settings › Browser › Site settings; wry's own delegate would grant them; see
//! `browser_ui`). It is made when the tab first loads a page and dropped when
//! the tab closes. Linux has no embedded page (Tauri's window there is a GTK box a child can't
//! be laid over); its tab opens pages in the system browser.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use brigadier_ipc::app::{
    BrowserBounds, BrowserClear, BrowserDownload, BrowserEvent, BrowserFound, BrowserOptions,
    BrowserSite, BrowserVisit, HistoryFilter, SitePermission,
};
use brigadier_ipc::protocol::{ErrorCode, IpcError};
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::time::{Duration, Instant};
use tauri::Manager;
use tauri::ipc::Channel;

use crate::browser_store;

fn failed(message: impl Into<String>) -> IpcError {
    IpcError {
        code: ErrorCode::Internal,
        message: message.into(),
    }
}

/// A page action only macOS's WebKit has (zoom, find, print, screenshots, reloading past the
/// cache); the tab shows them disabled elsewhere.
#[cfg(not(target_os = "macos"))]
fn unsupported() -> IpcError {
    IpcError {
        code: ErrorCode::Invalid,
        message: "the built-in browser can't do that on this platform".into(),
    }
}

/// The least and most a page zooms to: 30% and 300%.
const ZOOM_RANGE: (f64, f64) = (0.3, 3.0);

/// `zoom` within [`ZOOM_RANGE`]; 100% for anything that isn't a number.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn zoom_within(zoom: f64) -> f64 {
    if zoom.is_finite() {
        zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1)
    } else {
        1.0
    }
}

/// Counts the matches of `query` in the page's text as shown, whatever its case, and which of
/// them the selection (the match WebKit's find just selected) starts at: `[total, current]`.
/// Text is read as it renders: runs of spaces as one, and no match across two blocks. It stops
/// counting at 1000.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn count_script(query: &str) -> String {
    let query = serde_json::to_string(query).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(() => {{
  const query = {query}.replace(/\s+/g, " ").toLowerCase();
  const body = document.body;
  if (!body || !query) return [0, 0];
  const shown = new Map();
  const visible = (element) => {{
    let seen = shown.get(element);
    if (seen === undefined) {{
      seen = element.checkVisibility
        ? element.checkVisibility({{ visibilityProperty: true }})
        : element.getClientRects().length > 0;
      shown.set(element, seen);
    }}
    return seen;
  }};
  const blocks = new Map();
  const blockOf = (element) => {{
    const path = [];
    let cursor = element;
    let block = body;
    while (cursor && cursor !== body) {{
      const known = blocks.get(cursor);
      if (known) {{ block = known; break; }}
      path.push(cursor);
      const display = getComputedStyle(cursor).display;
      if (!display.startsWith("inline") && display !== "contents") {{ block = cursor; break; }}
      cursor = cursor.parentElement;
    }}
    for (const element of path) blocks.set(element, block);
    return block;
  }};
  const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
  const nodes = [];
  let text = "";
  let lastBlock = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {{
    const parent = node.parentElement;
    if (!parent || /^(SCRIPT|STYLE|NOSCRIPT|TEMPLATE)$/.test(parent.tagName) || !visible(parent)) continue;
    const block = blockOf(parent);
    if (lastBlock && block !== lastBlock) text += "\n";
    lastBlock = block;
    let chunk = node.data.replace(/\s+/g, " ").toLowerCase();
    const lead = chunk.startsWith(" ") && /[ \n]$/.test(text) ? 1 : 0;
    chunk = chunk.slice(lead);
    nodes.push({{ node, start: text.length, lead }});
    text += chunk;
  }}
  let at = -1;
  const selection = getSelection();
  if (selection && selection.rangeCount > 0) {{
    const range = selection.getRangeAt(0);
    const entry = nodes.find((item) => item.node === range.startContainer);
    if (entry) {{
      const before = entry.node.data.slice(0, range.startOffset).replace(/\s+/g, " ").length;
      at = entry.start + Math.max(0, before - entry.lead);
    }}
  }}
  let total = 0;
  let current = 0;
  for (let from = text.indexOf(query); from >= 0 && total < 1000; from = text.indexOf(query, from + query.length)) {{
    total += 1;
    if (from <= at) current = total;
  }}
  return [total, current];
}})()"#
    )
}

/// What the count script answered, as WebKit's JSON: `(total, current)`, nothing known when it
/// failed.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn counted(json: &str) -> (u32, u32) {
    serde_json::from_str::<(u32, u32)>(json).unwrap_or((0, 0))
}

/// ⌘← or ⌘→ from the Browser menu while the page has the keyboard: the menu takes the key
/// before the page, so in a text field the caret goes to the line's start or end (`side`) as
/// it would have; anywhere else the page goes back or forward (`go`). A field in a frame of
/// another site can't be told from outside, so nothing happens there.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", test)),
    allow(dead_code)
)]
fn unless_typing(side: &str, go: &str) -> String {
    format!(
        r#"(() => {{
  let view = window;
  let field = document.activeElement;
  while (field && field.tagName === "IFRAME") {{
    try {{ view = field.contentWindow; field = field.contentDocument.activeElement; }} catch {{ return; }}
    if (!view) return;
  }}
  const typing = field && (field.isContentEditable || field.tagName === "TEXTAREA" ||
    (field.tagName === "INPUT" && !/^(button|checkbox|color|file|hidden|image|radio|range|reset|submit)$/i.test(field.type)));
  if (typing) view.getSelection()?.modify("move", "{side}", "lineboundary");
  else {go};
}})()"#
    )
}

/// Leaves no match selected once the find bar closes.
#[cfg(target_os = "macos")]
const CLEAR_FIND: &str = "getSelection()?.removeAllRanges()";

/// Puts the caret where the selected match starts, for WebKit's find to look on from there.
#[cfg(target_os = "macos")]
const FIND_FROM_START: &str =
    "(() => { const s = getSelection(); if (s && s.rangeCount > 0) s.collapseToStart(); })()";

/// Whether the tab may show `url`: web pages, and the blank and blob pages they make.
/// Everything else (files, the app's own schemes, `data:`, `javascript:`) is refused.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn allowed(url: &str) -> bool {
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("http" | "https" | "about" | "blob"))
}

/// Run in every frame of the tab's pages before their own scripts: takes away screen capture
/// and speech recognition (which would listen through the app's own microphone use). On macOS
/// a page may ask for the camera and microphone, answered per site in `browser_ui`.
#[cfg(target_os = "macos")]
const NO_CAPTURE: &str = r#"(() => {
  const gone = (target, name) => {
    try {
      Object.defineProperty(target, name, { get: () => undefined, configurable: false });
    } catch {}
  };
  if (window.MediaDevices) gone(MediaDevices.prototype, "getDisplayMedia");
  for (const name of ["SpeechRecognition", "webkitSpeechRecognition"]) {
    gone(window, name);
  }
})();"#;

/// Windows has no per-site choices: the camera and microphone APIs go too, so the app's
/// dictation microphone is never asked for on a page's behalf.
#[cfg(target_os = "windows")]
const NO_CAPTURE: &str = r#"(() => {
  const gone = (target, name) => {
    try {
      Object.defineProperty(target, name, { get: () => undefined, configurable: false });
    } catch {}
  };
  if (window.MediaDevices) {
    for (const name of ["getUserMedia", "getDisplayMedia", "enumerateDevices"]) {
      gone(MediaDevices.prototype, name);
    }
  }
  for (const name of ["mediaDevices", "getUserMedia", "webkitGetUserMedia"]) {
    gone(Navigator.prototype, name);
  }
  for (const name of ["MediaDevices", "SpeechRecognition", "webkitSpeechRecognition"]) {
    gone(window, name);
  }
})();"#;

/// Rasterize in the page's own security context. CSP or CORS failures leave the globe icon;
/// no app-side network fetch, credentials, remote image CSP allowance or privileged bridge.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const FAVICON: &str = r#"(() => {
  if (window !== window.top) return;
  const send = window.ipc.postMessage.bind(window.ipc);
  let previous = "";
  const read = () => {
    const href = document.querySelector('link[rel~="icon"]')?.href || new URL('/favicon.ico', location.href).href;
    if (href === previous) return;
    previous = href;
    if (!/^(https?:|data:image\/)/i.test(href)) return;
    const image = new Image();
    image.crossOrigin = "anonymous";
    image.onload = () => {
      if (href !== previous) return;
      try {
        const canvas = document.createElement('canvas');
        canvas.width = canvas.height = 32;
        canvas.getContext('2d').drawImage(image, 0, 0, 32, 32);
        send(canvas.toDataURL('image/png'));
      } catch {}
    };
    image.src = href;
  };
  window.addEventListener('pageshow', (event) => {
    if (event.persisted) { previous = ""; read(); }
  });
  document.addEventListener('DOMContentLoaded', () => {
    read();
    if (document.head) new MutationObserver(read).observe(document.head, {
      childList: true, subtree: true, attributes: true, attributeFilter: ['href', 'rel']
    });
  }, { once: true });
})();"#;

/// Keys without matching menu accelerators, from every frame of the native page. These
/// messages only close the sender's find bar or zoom its page, and expose no app commands.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const PAGE_KEYS: &str = r#"(() => {
  const send = window.ipc.postMessage.bind(window.ipc);
  window.addEventListener("keydown", (event) => {
    if (!event.isTrusted) return;
    if (event.key === "Escape") send("browser:escape");
    // The menu's Cmd+= does not match Cmd+Shift+= (the + key) in a native page.
    if (event.metaKey && !event.ctrlKey && !event.altKey &&
        ((event.shiftKey && event.code === "Equal") || event.code === "NumpadAdd")) {
      event.preventDefault();
      send("browser:zoom-in");
    }
  }, true);
})();"#;

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn favicon_data(value: &str) -> bool {
    value.len() <= 16 * 1024
        && value
            .strip_prefix("data:image/png;base64,iVBORw0KGgo")
            .is_some_and(|data| {
                data.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
            })
}

/// The least time between two favicons a tab's page gets to the app.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const FAVICON_INTERVAL: Duration = Duration::from_millis(300);

/// What to do with a favicon a tab's page sent.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug, PartialEq)]
enum FaviconStep {
    /// Send it now.
    Send { url: String, data_url: String },
    /// Hold it: `FaviconLimit::flush` sends the latest held one after this long.
    Later(Duration),
    /// Drop it: the app has it already, or a held one goes in its place.
    Skip,
}

/// Keeps a page's favicons to the app to one per `FAVICON_INTERVAL`, the latest of those sent
/// in between, and none the same as the one before, whatever its scripts post.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Default)]
struct FaviconLimit {
    sent: Option<String>,
    at: Option<Instant>,
    held: Option<(String, String)>,
    flushing: bool,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl FaviconLimit {
    fn offer(&mut self, url: String, data_url: String, now: Instant) -> FaviconStep {
        if self.sent.as_ref() == Some(&data_url) {
            self.held = None;
            return FaviconStep::Skip;
        }
        match self.at.map(|at| now.saturating_duration_since(at)) {
            Some(since) if since < FAVICON_INTERVAL => {
                self.held = Some((url, data_url));
                if std::mem::replace(&mut self.flushing, true) {
                    FaviconStep::Skip
                } else {
                    FaviconStep::Later(FAVICON_INTERVAL - since)
                }
            }
            _ => {
                self.held = None;
                self.sent = Some(data_url.clone());
                self.at = Some(now);
                FaviconStep::Send { url, data_url }
            }
        }
    }

    /// Recheck the latest send time: a reset or immediate send may have moved the deadline.
    fn flush(&mut self, now: Instant) -> FaviconStep {
        self.flushing = false;
        match self.held.take() {
            Some((url, data_url)) => self.offer(url, data_url, now),
            None => FaviconStep::Skip,
        }
    }

    /// A new page is loading: the app clears the tab's favicon, so the next one goes.
    fn reset(&mut self) {
        self.sent = None;
        self.at = None;
        self.held = None;
    }
}

/// A tab's latest page in the browser's history: the page that finished loading, and the title
/// it showed meanwhile, which may come before or after.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Default)]
struct Visit {
    id: Option<i64>,
    title: String,
    /// A page is loading: a title now is the new page's, kept for its visit, never the
    /// last page's.
    loading: bool,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Visit {
    fn load(
        &mut self,
        store: &crate::browser_store::Store,
        url: &str,
        loading: bool,
        agent: bool,
        now: i64,
    ) -> rusqlite::Result<()> {
        self.loading = loading;
        if loading {
            self.title.clear();
            return Ok(());
        }
        if !web_page(url) {
            self.id = None;
            return Ok(());
        }
        let visited = store.visit(self.id, url, &self.title, agent, now);
        self.id = visited.as_ref().ok().copied();
        visited.map(drop)
    }

    fn titled(&mut self, store: &crate::browser_store::Store, title: &str) -> rusqlite::Result<()> {
        self.title = title.to_owned();
        match self.id {
            Some(id) if !self.loading => store.titled(id, title),
            _ => Ok(()),
        }
    }
}

/// `name` in `dir`, or with " (2)", " (3)"… before its extension while `taken` says that one is.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn free_name(dir: &Path, name: &std::ffi::OsStr, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let name = Path::new(name);
    let stem = name.file_stem().unwrap_or_default().to_string_lossy();
    let extension = name
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){extension}")))
        .find(|path| !taken(path))
        .unwrap_or(first)
}

/// Where a download goes, given where WebKit or WebView2 would put it (`suggested`, in the
/// system's Downloads folder): the user's folder when one is set and there, and with "Ask where
/// to save" (macOS) where they pick, `None` when they cancel.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn download_target(suggested: &Path) -> Option<PathBuf> {
    let options = DOWNLOADS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let name = suggested.file_name()?;
    let chosen = options
        .as_ref()
        .map(|options| PathBuf::from(&options.downloads_dir))
        .filter(|dir| dir.is_absolute() && dir.is_dir());
    let dir = chosen.or_else(|| suggested.parent().map(Path::to_path_buf))?;
    let target = free_name(&dir, name, Path::exists);
    #[cfg(target_os = "macos")]
    if options.is_some_and(|options| options.ask_downloads) {
        return crate::browser_ui::save_path(&target);
    }
    Some(target)
}

/// Whether `url` is a web page, which the system browser may open.
fn web_page(url: &str) -> bool {
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("http" | "https"))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod embedded {
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use brigadier_ipc::app::{BrowserBounds, BrowserEvent};
    use brigadier_ipc::protocol::IpcError;
    use tauri::ipc::Channel;
    use tauri::{AppHandle, Manager};
    use tauri_plugin_opener::OpenerExt;
    #[cfg(target_os = "windows")]
    use wry::NewWindowResponse;
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{PageLoadEvent, Rect, WebView, WebViewBuilder};

    use super::{
        FAVICON, FaviconLimit, FaviconStep, NO_CAPTURE, PAGE_KEYS, Visit, allowed, failed,
        favicon_data, web_page,
    };
    use crate::shell::MAIN_WINDOW;

    /// A tab's downloads under way, by address: their history entries and where they go.
    type Downloads = HashMap<String, VecDeque<(Option<i64>, PathBuf)>>;

    /// A tab's page.
    struct Tab {
        webview: WebView,
        /// The page's own WebKit UI delegate, which WebKit holds only weakly.
        #[cfg(target_os = "macos")]
        _ui: crate::browser_ui::BrowserUi,
    }

    thread_local! {
        /// The open tabs' pages by tab. Tauri runs the (non-async) browser commands on the
        /// main thread, the only one a webview may be used from.
        static BROWSERS: RefCell<HashMap<String, Tab>> = RefCell::new(HashMap::new());
    }

    /// A page's popup (`window.open`, `target=_blank`): a web page opens in the system
    /// browser, anything else nowhere.
    fn popup(app: &AppHandle, url: String) {
        if web_page(&url) {
            tracing::info!(%url, "Browser tab: popup sent to the system browser");
            let _ = app.opener().open_url(url, None::<&str>);
        }
    }

    fn rect(bounds: BrowserBounds) -> Rect {
        Rect {
            position: LogicalPosition::new(bounds.x, bounds.y).into(),
            size: LogicalSize::new(bounds.width.max(1.0), bounds.height.max(1.0)).into(),
        }
    }

    pub fn open(
        app: &AppHandle,
        id: String,
        url: String,
        bounds: BrowserBounds,
        agent: bool,
        events: Channel<BrowserEvent>,
    ) -> Result<(), IpcError> {
        if BROWSERS.with_borrow(|browsers| browsers.contains_key(&id)) {
            return navigate(&id, &url, bounds);
        }
        let window = app
            .get_webview_window(MAIN_WINDOW)
            .ok_or_else(|| failed("the window is gone"))?;
        let opener = app.clone();
        let (on_load, on_title, on_blocked, on_download, on_key, on_favicon) = (
            events.clone(),
            events.clone(),
            events.clone(),
            events.clone(),
            events.clone(),
            events,
        );
        let downloads: Rc<RefCell<Downloads>> = Rc::default();
        let finished = downloads.clone();
        let limit = Arc::new(Mutex::new(FaviconLimit::default()));
        let visit = Arc::new(Mutex::new(Visit::default()));
        let title_visit = visit.clone();
        let load_limit = limit.clone();
        let saved = super::data_dir();
        // Windows: the browser's folder in Brigadier's data folder, apart from the app's webview.
        #[cfg(target_os = "windows")]
        let mut context = wry::WebContext::new(saved.map(|dir| dir.join("webview2")));
        #[cfg(target_os = "windows")]
        let builder = WebViewBuilder::new_with_web_context(&mut context);
        #[cfg(not(target_os = "windows"))]
        let builder = WebViewBuilder::new();
        #[cfg(target_os = "macos")]
        let builder = match saved {
            Some(dir) => {
                use wry::WebViewBuilderExtDarwin;
                builder.with_data_store_identifier(super::store_identifier(dir))
            }
            None => builder,
        };
        let builder = builder
            .with_url(&url)
            .with_bounds(rect(bounds))
            .with_visible(false)
            // Without a data folder (never in the app) nothing is kept.
            .with_incognito(saved.is_none())
            .with_initialization_script_for_main_only(NO_CAPTURE, false)
            .with_initialization_script_for_main_only(FAVICON, true)
            .with_initialization_script_for_main_only(PAGE_KEYS, false)
            .with_ipc_handler(move |request| {
                let url = request.uri().to_string();
                let data_url = request.into_body();
                if data_url == "browser:escape" {
                    let _ = on_key.send(BrowserEvent::Escape);
                    return;
                }
                if data_url == "browser:zoom-in" {
                    let _ = on_key.send(BrowserEvent::ZoomIn);
                    return;
                }
                if !web_page(&url) || !favicon_data(&data_url) {
                    return;
                }
                // The lock is held while sending, so a page load's reset and `Load` can't fall
                // between a favicon's step and its event.
                let mut held = limit.lock().expect("favicon lock");
                match held.offer(url, data_url, Instant::now()) {
                    FaviconStep::Send { url, data_url } => {
                        let _ = on_favicon.send(BrowserEvent::Favicon { url, data_url });
                    }
                    FaviconStep::Later(mut wait) => {
                        let (limit, on_favicon) = (limit.clone(), on_favicon.clone());
                        tauri::async_runtime::spawn(async move {
                            loop {
                                tokio::time::sleep(wait).await;
                                let mut held = limit.lock().expect("favicon lock");
                                match held.flush(Instant::now()) {
                                    FaviconStep::Later(remaining) => wait = remaining,
                                    FaviconStep::Send { url, data_url } => {
                                        let _ = on_favicon
                                            .send(BrowserEvent::Favicon { url, data_url });
                                        break;
                                    }
                                    FaviconStep::Skip => break,
                                }
                            }
                        });
                    }
                    FaviconStep::Skip => {}
                }
            })
            .with_back_forward_navigation_gestures(true)
            .with_navigation_handler(move |url| {
                let ok = allowed(&url);
                if !ok {
                    let _ = on_blocked.send(BrowserEvent::Blocked { url });
                }
                ok
            })
            .with_download_started_handler(move |url, path| {
                let Some(target) = super::download_target(path) else {
                    return false;
                };
                let now = crate::browser_store::now_ms();
                let id = crate::browser_store::with(|store| {
                    store.download_started(&url, &target.to_string_lossy(), now)
                });
                path.clone_from(&target);
                downloads
                    .borrow_mut()
                    .entry(url)
                    .or_default()
                    .push_back((id, target));
                true
            })
            .with_download_completed_handler(move |url, _path, ok| {
                // macOS never says where it saved; it's where the start was told to.
                let Some((id, path)) = finished
                    .borrow_mut()
                    .get_mut(&url)
                    .and_then(VecDeque::pop_front)
                else {
                    return;
                };
                if let Some(id) = id {
                    crate::browser_store::with(|store| store.download_finished(id, ok));
                }
                let _ = on_download.send(BrowserEvent::Downloaded {
                    path: path.to_string_lossy().into_owned(),
                    ok,
                });
            })
            .with_on_page_load_handler(move |event, url| {
                let loading = matches!(event, PageLoadEvent::Started);
                let mut held = load_limit.lock().expect("favicon lock");
                if loading {
                    held.reset();
                }
                drop(held);
                crate::browser_store::with(|store| {
                    let now = crate::browser_store::now_ms();
                    visit
                        .lock()
                        .expect("visit lock")
                        .load(store, &url, loading, agent, now)
                });
                let _ = on_load.send(BrowserEvent::Load { url, loading });
            })
            .with_document_title_changed_handler(move |title| {
                crate::browser_store::with(|store| {
                    title_visit
                        .lock()
                        .expect("visit lock")
                        .titled(store, &title)
                });
                let _ = on_title.send(BrowserEvent::Title { title });
            });
        // macOS: `browser_ui`'s delegate takes the popups instead.
        #[cfg(target_os = "windows")]
        let builder = builder.with_new_window_req_handler(move |url, _features| {
            popup(&opener, url);
            NewWindowResponse::Deny
        });
        let webview = builder
            .build_as_child(&window)
            .map_err(|err| failed(format!("could not make the browser: {err}")))?;
        let tab = Tab {
            #[cfg(target_os = "macos")]
            _ui: crate::browser_ui::install(&webview, move |url| popup(&opener, url)),
            webview,
        };
        BROWSERS.with_borrow_mut(|browsers| browsers.insert(id, tab));
        Ok(())
    }

    fn with_webview(
        id: &str,
        action: impl FnOnce(&WebView) -> wry::Result<()>,
    ) -> Result<(), IpcError> {
        BROWSERS.with_borrow(|browsers| match browsers.get(id) {
            Some(tab) => action(&tab.webview).map_err(|err| failed(err.to_string())),
            // Nothing loaded yet: nothing to move or steer.
            None => Ok(()),
        })
    }

    pub fn navigate(id: &str, url: &str, bounds: BrowserBounds) -> Result<(), IpcError> {
        BROWSERS.with_borrow(|browsers| {
            let webview = &browsers
                .get(id)
                .ok_or_else(|| failed("the browser's page is gone"))?
                .webview;
            webview
                .set_bounds(rect(bounds))
                .and_then(|()| webview.load_url(url))
                .map_err(|err| failed(format!("could not open it: {err}")))
        })
    }

    pub fn place(id: &str, bounds: Option<BrowserBounds>) -> Result<(), IpcError> {
        with_webview(id, |webview| match bounds {
            Some(bounds) => webview
                .set_bounds(rect(bounds))
                .and_then(|()| webview.set_visible(true)),
            None => webview.set_visible(false),
        })
    }

    pub fn go(id: &str, action: &str) -> Result<(), IpcError> {
        let script = match action {
            "back" => "history.back()",
            "forward" => "history.forward()",
            "reload" => "location.reload()",
            "stop" => "stop()",
            "back-unless-typing" => &super::unless_typing("left", "history.back()"),
            "forward-unless-typing" => &super::unless_typing("right", "history.forward()"),
            "force-reload" => return force_reload(id),
            other => return Err(failed(format!("unknown browser action {other}"))),
        };
        with_webview(id, |webview| webview.evaluate_script(script))
    }

    #[cfg(target_os = "macos")]
    fn force_reload(id: &str) -> Result<(), IpcError> {
        with_webview(id, |webview| {
            crate::browser_ui::reload_from_origin(webview);
            Ok(())
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn force_reload(_id: &str) -> Result<(), IpcError> {
        Err(super::unsupported())
    }

    /// Runs `action` with the tab's page; an error when it has none.
    #[cfg(target_os = "macos")]
    pub fn with_page<T>(id: &str, action: impl FnOnce(&WebView) -> T) -> Result<T, IpcError> {
        BROWSERS.with_borrow(|browsers| {
            browsers
                .get(id)
                .map(|tab| action(&tab.webview))
                .ok_or_else(|| failed("the browser's page is gone"))
        })
    }

    /// Gives the keyboard to the tab's page.
    pub fn focus(id: &str) -> Result<(), IpcError> {
        with_webview(id, WebView::focus)
    }

    pub fn close(id: &str) {
        BROWSERS.with_borrow_mut(|browsers| browsers.remove(id));
    }

    pub fn close_all() {
        BROWSERS.with_borrow_mut(HashMap::clear);
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod embedded {
    use brigadier_ipc::app::{BrowserBounds, BrowserEvent};
    use brigadier_ipc::protocol::IpcError;
    use tauri::AppHandle;
    use tauri::ipc::Channel;

    use super::failed;

    pub fn open(
        _app: &AppHandle,
        _id: String,
        _url: String,
        _bounds: BrowserBounds,
        _agent: bool,
        _events: Channel<BrowserEvent>,
    ) -> Result<(), IpcError> {
        Err(failed("pages open in the system browser on this platform"))
    }

    pub fn navigate(_id: &str, _url: &str, _bounds: BrowserBounds) -> Result<(), IpcError> {
        Err(failed("pages open in the system browser on this platform"))
    }

    pub fn place(_id: &str, _bounds: Option<BrowserBounds>) -> Result<(), IpcError> {
        Ok(())
    }

    pub fn go(_id: &str, _action: &str) -> Result<(), IpcError> {
        Ok(())
    }

    pub fn focus(_id: &str) -> Result<(), IpcError> {
        Ok(())
    }

    pub fn close(_id: &str) {}

    pub fn close_all() {}
}

/// The built-in browser's own folder in Brigadier's data folder, set once at start: its saved
/// store is named from it, and its history kept in it.
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Keeps the browser's saved data with Brigadier's data folder `data_dir`.
pub fn set_data_dir(data_dir: &Path) {
    let _ = DATA_DIR.set(data_dir.join("browser"));
}

/// The browser's folder, once the app has set it.
pub fn data_dir() -> Option<&'static Path> {
    DATA_DIR.get().map(PathBuf::as_path)
}

/// The WebKit data store that holds the pages' cookies and site data for the browser folder
/// `dir`: the same folder always gets the same store, and another (a test app's) never shares it.
#[cfg(any(target_os = "macos", test))]
pub fn store_identifier(dir: &Path) -> [u8; 16] {
    let mut hasher = blake3::Hasher::new_derive_key("brigadier browser data store");
    hasher.update(dir.as_os_str().as_encoded_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    id
}

/// Whether the built-in browser exists (Settings › Browser): on until the app says otherwise.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// The user's download choices, as the app last sent them.
static DOWNLOADS: Mutex<Option<BrowserOptions>> = Mutex::new(None);

/// Refuses a page while the browser is off: whatever asks, nothing opens in it.
pub fn check_enabled() -> Result<(), IpcError> {
    if ENABLED.load(Ordering::Acquire) {
        Ok(())
    } else {
        Err(IpcError {
            code: ErrorCode::Invalid,
            message: "Brigadier's browser is off (Settings › Browser)".into(),
        })
    }
}

/// The app's Settings › Browser choices: off drops every tab's page and the browser's menu
/// items; the download choices apply to the next download.
#[tauri::command]
pub fn browser_configure(app: tauri::AppHandle, options: BrowserOptions) {
    let was = ENABLED.swap(options.enabled, Ordering::AcqRel);
    if !options.enabled {
        embedded::close_all();
    }
    if was != options.enabled {
        #[cfg(target_os = "macos")]
        if let Err(err) = crate::shell::browser_menu(&app, options.enabled) {
            tracing::warn!(error = %err, "could not change the browser's menu items");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = &app;
    *DOWNLOADS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(options);
}

fn web_page_only(url: &str) -> Result<(), IpcError> {
    if web_page(url) {
        Ok(())
    } else {
        Err(IpcError {
            code: ErrorCode::Invalid,
            message: format!("only web pages open here: {url}"),
        })
    }
}

/// Makes the tab `id`'s webview showing `url` at `bounds`; its page events arrive on
/// `events`. `agent`: the tab was opened from something an agent gave (its history says so). For a tab that has one, it goes to `url` (see `browser_navigate`).
#[tauri::command]
pub fn browser_open(
    app: tauri::AppHandle,
    id: String,
    url: String,
    bounds: BrowserBounds,
    agent: Option<bool>,
    events: Channel<BrowserEvent>,
) -> Result<(), IpcError> {
    check_enabled()?;
    web_page_only(&url)?;
    embedded::open(&app, id, url, bounds, agent.unwrap_or(false), events)
}

/// Shows `url` in the tab `id`'s webview, at `bounds`.
#[tauri::command]
pub fn browser_navigate(id: String, url: String, bounds: BrowserBounds) -> Result<(), IpcError> {
    check_enabled()?;
    web_page_only(&url)?;
    embedded::navigate(&id, &url, bounds)
}

/// Moves the tab's page to `bounds`, or hides it (`None`): the tab was hidden or covered.
#[tauri::command]
pub fn browser_place(id: String, bounds: Option<BrowserBounds>) -> Result<(), IpcError> {
    embedded::place(&id, bounds)
}

/// Back, forward, reload, reload past the cache (`force-reload`, macOS) or stop in the tab's
/// page; `back-unless-typing` and `forward-unless-typing` are ⌘← and ⌘→ (see `unless_typing`).
#[tauri::command]
pub fn browser_go(id: String, action: String) -> Result<(), IpcError> {
    embedded::go(&id, &action)
}

/// Runs `action` with the tab's page on the main thread, and waits for what it sends.
#[cfg(target_os = "macos")]
async fn on_page<T: Send + 'static>(
    app: &tauri::AppHandle,
    id: String,
    action: impl FnOnce(&wry::WebView, tokio::sync::oneshot::Sender<T>) + Send + 'static,
) -> Result<T, IpcError> {
    let (sent, answer) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        // Without a page, `sent` goes unanswered and the wait below ends in an error.
        let _ = embedded::with_page(&id, |webview| action(webview, sent));
    })
    .map_err(|err| failed(err.to_string()))?;
    answer
        .await
        .map_err(|_| failed("the browser's page is gone"))
}

/// The tab's page zoom (1.0 being 100%): set to `zoom` (within 30% to 300%) first, when given.
/// The page keeps it from page to page while the tab is open.
#[tauri::command]
pub fn browser_zoom(id: String, zoom: Option<f64>) -> Result<f64, IpcError> {
    #[cfg(target_os = "macos")]
    {
        embedded::with_page(&id, |webview| {
            if let Some(zoom) = zoom {
                crate::browser_ui::set_page_zoom(webview, zoom_within(zoom));
            }
            crate::browser_ui::page_zoom(webview)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (id, zoom);
        Err(unsupported())
    }
}

/// Finds `query` in the tab's page, whatever its case, going round at the end, with its
/// matches counted. `step`: `type` (the query changed) looks from where the match selected
/// starts, so it stays while it still matches; `next` and `previous` from it on. An empty
/// `query` leaves no match selected (the find bar closed).
#[tauri::command]
pub async fn browser_find(
    app: tauri::AppHandle,
    id: String,
    query: String,
    step: String,
) -> Result<BrowserFound, IpcError> {
    #[cfg(target_os = "macos")]
    {
        let backwards = match step.as_str() {
            "type" | "next" => false,
            "previous" => true,
            other => return Err(failed(format!("unknown find step {other}"))),
        };
        if query.is_empty() {
            on_page(&app, id, |webview, sent| {
                let _ = webview.evaluate_script(CLEAR_FIND);
                let _ = sent.send(());
            })
            .await?;
            return Ok(BrowserFound::default());
        }
        let script = count_script(&query);
        let again = step == "type";
        let found = on_page(&app, id.clone(), move |webview, sent| {
            // WebKit looks after the selection: from its start, the match typed so far stays.
            if again {
                let _ = webview.evaluate_script(FIND_FROM_START);
            }
            crate::browser_ui::find(webview, &query, backwards, move |found| {
                let _ = sent.send(found);
            });
        })
        .await?;
        if !found {
            return Ok(BrowserFound::default());
        }
        // The count is a bonus: a page that won't run it still has its match.
        let json = on_page(&app, id, move |webview, sent| {
            let sent = Mutex::new(Some(sent));
            let _ = webview.evaluate_script_with_callback(&script, move |json| {
                if let Some(sent) = sent
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    let _ = sent.send(json);
                }
            });
        })
        .await
        .unwrap_or_default();
        let (total, current) = counted(&json);
        Ok(BrowserFound {
            found,
            current: current.min(total),
            total,
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id, query, step);
        Err(unsupported())
    }
}

/// What the page reads to the chat beside it: its address, title, text and selection.
const READ_PAGE: &str = r#"({
  url: location.href,
  title: document.title,
  text: (document.body ? document.body.innerText : "").slice(0, 200000),
  selection: String(getSelection() || "").slice(0, 20000)
})"#;

/// What the tab's page shows, for the chat beside it.
#[tauri::command]
pub async fn browser_read(
    app: tauri::AppHandle,
    id: String,
) -> Result<brigadier_ipc::app::BrowserText, IpcError> {
    #[cfg(target_os = "macos")]
    {
        let json = on_page(&app, id, move |webview, sent| {
            let sent = Mutex::new(Some(sent));
            let _ = webview.evaluate_script_with_callback(READ_PAGE, move |json| {
                if let Some(sent) = sent
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                {
                    let _ = sent.send(json);
                }
            });
        })
        .await?;
        serde_json::from_str(&json).map_err(|err| failed(format!("the page didn't read: {err}")))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id, READ_PAGE);
        Err(unsupported())
    }
}

/// Opens the print panel for the tab's page.
#[tauri::command]
pub fn browser_print(id: String) -> Result<(), IpcError> {
    #[cfg(target_os = "macos")]
    {
        if embedded::with_page(&id, crate::browser_ui::print)? {
            Ok(())
        } else {
            Err(failed("the page isn't in the window"))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        Err(unsupported())
    }
}

/// Puts a picture of the tab's page, as shown, on the clipboard.
#[tauri::command]
pub async fn browser_snapshot(app: tauri::AppHandle, id: String) -> Result<(), IpcError> {
    #[cfg(target_os = "macos")]
    {
        let copied = on_page(&app, id, |webview, sent| {
            crate::browser_ui::copy_snapshot(webview, move |copied| {
                let _ = sent.send(copied);
            });
        })
        .await?;
        if copied {
            Ok(())
        } else {
            Err(failed("the screenshot couldn't be taken"))
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, id);
        Err(unsupported())
    }
}

/// Gives the keyboard to the tab `id`'s page, or with none back to the app's own: a native
/// page keeps it from the app's fields until one of them asks.
#[tauri::command]
pub fn browser_focus(app: tauri::AppHandle, id: Option<String>) -> Result<(), IpcError> {
    match id {
        Some(id) => embedded::focus(&id),
        None => app
            .get_webview_window(crate::shell::MAIN_WINDOW)
            .ok_or_else(|| failed("the window is gone"))?
            .as_ref()
            .set_focus()
            .map_err(|err| failed(err.to_string())),
    }
}

/// The built-in browser's history, newest first: the last visit shown (`before_ms`,
/// `before_id`) pages through it.
#[tauri::command]
pub fn browser_history(
    query: String,
    filter: HistoryFilter,
    before_ms: Option<i64>,
    before_id: Option<i64>,
    limit: u32,
) -> Vec<BrowserVisit> {
    let before = before_ms.map(|at_ms| (at_ms, before_id.unwrap_or(i64::MAX)));
    browser_store::with(|store| store.history(&query, filter, before, limit)).unwrap_or_default()
}

/// Removes pages from the browser's history.
#[tauri::command]
pub fn browser_history_remove(ids: Vec<i64>) -> Result<(), IpcError> {
    browser_store::with(|store| store.remove_visits(&ids))
        .ok_or_else(|| failed("the browser's history can't be changed now"))
}

/// The browser's downloads, newest first, whose address or file holds `query`.
#[tauri::command]
pub fn browser_downloads(query: String) -> Vec<BrowserDownload> {
    browser_store::with(|store| store.downloads(&query)).unwrap_or_default()
}

/// Forgets downloads (the files stay): `ids`, or every finished one with none.
#[tauri::command]
pub fn browser_downloads_remove(ids: Option<Vec<i64>>) -> Result<(), IpcError> {
    browser_store::with(|store| store.remove_downloads(ids.as_deref(), None))
        .ok_or_else(|| failed("the download history can't be changed now"))
}

/// The sites that asked for the camera or microphone, with the user's choices.
#[tauri::command]
pub fn browser_sites() -> Vec<BrowserSite> {
    browser_store::with(|store| store.sites()).unwrap_or_default()
}

/// Sets what the site `origin` may use: the camera, the microphone, or both.
#[tauri::command]
pub fn browser_site_set(
    origin: String,
    camera: Option<SitePermission>,
    microphone: Option<SitePermission>,
) -> Result<(), IpcError> {
    browser_store::with(|store| {
        if let Some(choice) = camera {
            store.set_site(&origin, browser_store::Device::Camera, choice)?;
        }
        if let Some(choice) = microphone {
            store.set_site(&origin, browser_store::Device::Microphone, choice)?;
        }
        Ok(())
    })
    .ok_or_else(|| failed("the site settings can't be changed now"))
}

/// Runs `action` with the browser's data store on the main thread, and waits for its answer.
#[cfg(target_os = "macos")]
pub async fn on_store<T: Send + 'static>(
    app: &tauri::AppHandle,
    action: impl FnOnce([u8; 16], Box<dyn FnOnce(T)>) + Send + 'static,
) -> Result<T, IpcError> {
    let identifier = data_dir()
        .map(store_identifier)
        .ok_or_else(|| failed("the browser has no data folder yet"))?;
    let (sent, answer) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        action(
            identifier,
            Box::new(move |value| {
                let _ = sent.send(value);
            }),
        );
    })
    .map_err(|err| failed(err.to_string()))?;
    answer
        .await
        .map_err(|_| failed("the browser's data store didn't answer"))
}

/// Settings › Browser › Clear browsing data: removes what `clear` picks, changed since its
/// time. Cookies, site data and the cache are WebKit's (macOS only; elsewhere the page doesn't
/// offer them); history, download history and site choices are the browser's own.
#[tauri::command]
pub async fn browser_clear(app: tauri::AppHandle, clear: BrowserClear) -> Result<(), IpcError> {
    let since = clear.since_ms;
    let kept = browser_store::with(|store| {
        if clear.history {
            store.clear_history(since)?;
        }
        if clear.downloads {
            store.remove_downloads(None, since)?;
        }
        if clear.sites {
            store.clear_sites(since)?;
        }
        Ok(())
    });
    if (clear.history || clear.downloads || clear.sites) && kept.is_none() {
        return Err(failed("the browser's history can't be changed now"));
    }
    #[cfg(target_os = "macos")]
    if clear.cookies || clear.cache {
        let (site_data, cache) = (clear.cookies, clear.cache);
        let done = on_store(&app, move |identifier, done| {
            crate::browser_ui::remove_data(identifier, site_data, cache, since, done);
        })
        .await?;
        if !done {
            return Err(failed("the browser's data couldn't be cleared"));
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = &app;
    Ok(())
}

/// How many sites the browser keeps cookies or other data for (macOS; `None` elsewhere).
#[tauri::command]
pub async fn browser_site_count(app: tauri::AppHandle) -> Result<Option<u32>, IpcError> {
    #[cfg(target_os = "macos")]
    {
        let count = on_store(&app, crate::browser_ui::count_sites).await?;
        Ok(count.map(|count| u32::try_from(count).unwrap_or(u32::MAX)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &app;
        Ok(None)
    }
}

/// Drops every tab's page (on the main thread): the app's own page started again.
pub fn close_all() {
    embedded::close_all();
}

/// The tab closed: its page goes (what it saved stays in the browser's store).
#[tauri::command]
pub fn browser_close(id: String) {
    embedded::close(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_data_folder_has_its_own_lasting_store() {
        let real = store_identifier(Path::new("/Users/u/Library/Application Support/x/browser"));
        assert_eq!(
            real,
            store_identifier(Path::new("/Users/u/Library/Application Support/x/browser"))
        );
        assert_ne!(real, store_identifier(Path::new("/tmp/scratch/browser")));
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn a_title_while_the_next_page_loads_is_that_pages() {
        use crate::browser_store::Store;
        use brigadier_ipc::app::HistoryFilter;
        let store = Store::in_memory();
        let mut visit = Visit::default();
        let title = |store: &Store| {
            store
                .history("", HistoryFilter::All, None, 10)
                .unwrap()
                .into_iter()
                .map(|v| (v.url, v.title))
                .collect::<Vec<_>>()
        };
        visit
            .load(&store, "https://a.test/", true, false, 1)
            .unwrap();
        visit
            .load(&store, "https://a.test/", false, false, 2)
            .unwrap();
        visit.titled(&store, "A").unwrap();
        // B shows its title before it finishes loading: A keeps its own.
        visit
            .load(&store, "https://b.test/", true, false, 3)
            .unwrap();
        visit.titled(&store, "B").unwrap();
        assert_eq!(title(&store), [("https://a.test/".into(), "A".into())]);
        visit
            .load(&store, "https://b.test/", false, false, 4)
            .unwrap();
        // A reload takes the title it showed.
        visit
            .load(&store, "https://b.test/", true, false, 5)
            .unwrap();
        visit.titled(&store, "B, again").unwrap();
        visit
            .load(&store, "https://b.test/", false, false, 6)
            .unwrap();
        assert_eq!(
            title(&store),
            [
                ("https://b.test/".into(), "B, again".into()),
                ("https://a.test/".into(), "A".into()),
            ]
        );
    }

    #[test]
    fn a_download_takes_a_free_name_in_its_folder() {
        let dir = Path::new("/d");
        let taken = |names: &'static [&'static str]| {
            move |path: &Path| names.iter().any(|name| Path::new("/d").join(name) == path)
        };
        let name = std::ffi::OsStr::new("report.pdf");
        assert_eq!(free_name(dir, name, taken(&[])), Path::new("/d/report.pdf"));
        assert_eq!(
            free_name(dir, name, taken(&["report.pdf", "report (2).pdf"])),
            Path::new("/d/report (3).pdf")
        );
        assert_eq!(
            free_name(dir, std::ffi::OsStr::new("notes"), taken(&["notes"])),
            Path::new("/d/notes (2)")
        );
    }

    #[test]
    fn zoom_stays_between_30_and_300_percent() {
        assert_eq!(zoom_within(1.1), 1.1);
        assert_eq!(zoom_within(0.1), 0.3);
        assert_eq!(zoom_within(5.0), 3.0);
        assert_eq!(zoom_within(f64::NAN), 1.0);
        assert_eq!(zoom_within(f64::INFINITY), 1.0);
    }

    #[test]
    fn the_count_script_takes_the_query_as_a_string_and_its_answer_reads_safely() {
        let script = count_script("say \"hi\" </script> ${x}");
        assert!(script.contains(r#"const query = "say \"hi\" </script> ${x}".replace"#));
        assert!(script.starts_with("(() => {") && script.ends_with("})()"));
        assert_eq!(counted("[12,3]"), (12, 3));
        assert_eq!(counted(""), (0, 0));
        assert_eq!(counted("null"), (0, 0));
        assert_eq!(counted("[-1,2]"), (0, 0));
    }

    #[test]
    fn command_arrows_move_the_caret_in_a_field_and_go_elsewhere() {
        let back = unless_typing("left", "history.back()");
        assert!(back.contains(r#"modify("move", "left", "lineboundary")"#));
        assert!(back.contains("else history.back();"));
        assert!(unless_typing("right", "history.forward()").contains(r#""right""#));
    }

    #[test]
    fn nothing_opens_while_the_browser_is_off() {
        ENABLED.store(false, Ordering::Release);
        assert!(check_enabled().is_err());
        ENABLED.store(true, Ordering::Release);
        assert!(check_enabled().is_ok());
    }

    fn offer(limit: &mut FaviconLimit, data_url: &str, now: Instant) -> FaviconStep {
        limit.offer("https://example.com/".into(), data_url.into(), now)
    }

    fn send(data_url: &str) -> FaviconStep {
        FaviconStep::Send {
            url: "https://example.com/".into(),
            data_url: data_url.into(),
        }
    }

    #[test]
    fn favicon_limit_drops_repeats_and_coalesces_bursts_to_the_latest() {
        let start = Instant::now();
        let mut limit = FaviconLimit::default();
        assert_eq!(offer(&mut limit, "a", start), send("a"));
        // The same icon again is dropped, however late.
        assert_eq!(
            offer(&mut limit, "a", start + FAVICON_INTERVAL * 2),
            FaviconStep::Skip
        );

        // A burst within the interval: one wait, then only the latest goes.
        let soon = start + FAVICON_INTERVAL / 3;
        assert_eq!(
            offer(&mut limit, "b", soon),
            FaviconStep::Later(FAVICON_INTERVAL - FAVICON_INTERVAL / 3)
        );
        assert_eq!(offer(&mut limit, "c", soon), FaviconStep::Skip);
        assert_eq!(offer(&mut limit, "d", soon), FaviconStep::Skip);
        let flushed = start + FAVICON_INTERVAL;
        assert_eq!(limit.flush(flushed), send("d"));
        assert_eq!(limit.flush(flushed), FaviconStep::Skip);

        // Settling back on the icon already sent cancels the held one.
        let next = flushed + FAVICON_INTERVAL / 2;
        assert!(matches!(
            offer(&mut limit, "e", next),
            FaviconStep::Later(_)
        ));
        assert_eq!(offer(&mut limit, "d", next), FaviconStep::Skip);
        assert_eq!(limit.flush(flushed + FAVICON_INTERVAL), FaviconStep::Skip);

        // Once the interval has passed, a new icon goes at once.
        assert_eq!(
            offer(&mut limit, "f", flushed + FAVICON_INTERVAL * 2),
            send("f")
        );
    }

    #[test]
    fn favicon_timer_after_reset_waits_for_the_new_pages_interval() {
        let start = Instant::now();
        let mut limit = FaviconLimit::default();
        assert_eq!(offer(&mut limit, "a", start), send("a"));
        assert_eq!(
            offer(&mut limit, "b", start + Duration::from_millis(100)),
            FaviconStep::Later(Duration::from_millis(200))
        );

        limit.reset();
        let sent = start + Duration::from_millis(250);
        assert_eq!(offer(&mut limit, "a", sent), send("a"));
        assert_eq!(
            offer(&mut limit, "c", sent + Duration::from_millis(10)),
            FaviconStep::Skip
        );
        assert_eq!(
            limit.flush(start + FAVICON_INTERVAL),
            FaviconStep::Later(Duration::from_millis(250))
        );
        // The existing timer still owns the latest icon in the new page's burst.
        assert_eq!(
            offer(&mut limit, "d", sent + Duration::from_millis(100)),
            FaviconStep::Skip
        );
        assert_eq!(limit.flush(sent + FAVICON_INTERVAL), send("d"));
        assert_eq!(limit.flush(sent + FAVICON_INTERVAL), FaviconStep::Skip);
    }

    #[test]
    fn delayed_favicon_timer_waits_after_an_immediate_send() {
        let start = Instant::now();
        let mut limit = FaviconLimit::default();
        assert_eq!(offer(&mut limit, "a", start), send("a"));
        assert_eq!(
            offer(&mut limit, "b", start + Duration::from_millis(100)),
            FaviconStep::Later(Duration::from_millis(200))
        );

        let sent = start + Duration::from_millis(400);
        assert_eq!(offer(&mut limit, "c", sent), send("c"));
        assert_eq!(
            offer(&mut limit, "d", sent + Duration::from_millis(1)),
            FaviconStep::Skip
        );
        assert_eq!(
            limit.flush(sent + Duration::from_millis(2)),
            FaviconStep::Later(Duration::from_millis(298))
        );
        assert_eq!(
            limit.flush(sent + FAVICON_INTERVAL - Duration::from_millis(1)),
            FaviconStep::Later(Duration::from_millis(1))
        );
        assert_eq!(limit.flush(sent + FAVICON_INTERVAL), send("d"));
        assert_eq!(limit.flush(sent + FAVICON_INTERVAL), FaviconStep::Skip);
    }
}
