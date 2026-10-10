//! The side panel's Browser tab: a system webview laid over the tab's area of the window.
//!
//! It is a plain `wry` webview, not a Tauri one: Tauri gives each of its webviews the app's IPC
//! bridge, init scripts and custom protocols, and a web page must get none of that. This one
//! has no app IPC bridge or custom protocols. Its metadata-only handler accepts a bounded PNG
//! favicon for this tab, with no commands or access to the app. `NO_CAPTURE` takes the
//! microphone, camera and speech APIs away; it keeps its cookies and
//! storage in memory only, separate from the app's own webview; it opens only web pages, sends
//! popups to the system browser, and on macOS denies the page the camera and microphone (wry's
//! own delegate would grant them; see `browser_ui`). It is made when the tab first loads a page and dropped
//! when the tab closes. Linux has no embedded page (Tauri's window there is a GTK box a child
//! can't be laid over); its tab opens pages in the system browser.

use brigadier_ipc::app::{BrowserBounds, BrowserEvent};
use brigadier_ipc::protocol::{ErrorCode, IpcError};
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::time::{Duration, Instant};
use tauri::ipc::Channel;

fn failed(message: impl Into<String>) -> IpcError {
    IpcError {
        code: ErrorCode::Internal,
        message: message.into(),
    }
}

/// Whether the tab may show `url`: web pages, and the blank and blob pages they make.
/// Everything else (files, the app's own schemes, `data:`, `javascript:`) is refused.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn allowed(url: &str) -> bool {
    let scheme = url
        .split_once(':')
        .map(|(scheme, _)| scheme.to_ascii_lowercase());
    matches!(scheme.as_deref(), Some("http" | "https" | "about" | "blob"))
}

/// Run in every frame of the tab's pages before their own scripts: takes away the
/// microphone, camera and speech recognition APIs. The app itself may use the microphone
/// (dictation), and asking for it here would make the system ask the user on a page's behalf;
/// a page that still asks is refused (macOS: `browser_ui`; Windows: WebView2 asks the user).
#[cfg(any(target_os = "macos", target_os = "windows"))]
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
    use std::collections::HashMap;
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
        FAVICON, FaviconLimit, FaviconStep, NO_CAPTURE, allowed, failed, favicon_data, web_page,
    };
    use crate::shell::MAIN_WINDOW;

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
        events: Channel<BrowserEvent>,
    ) -> Result<(), IpcError> {
        if BROWSERS.with_borrow(|browsers| browsers.contains_key(&id)) {
            return navigate(&id, &url, bounds);
        }
        let window = app
            .get_webview_window(MAIN_WINDOW)
            .ok_or_else(|| failed("the window is gone"))?;
        let opener = app.clone();
        let (on_load, on_title, on_blocked, on_download, on_favicon) = (
            events.clone(),
            events.clone(),
            events.clone(),
            events.clone(),
            events,
        );
        let limit = Arc::new(Mutex::new(FaviconLimit::default()));
        let load_limit = limit.clone();
        // Windows keeps even a private page's browser process data in a folder; give this tab
        // its own, apart from the app's webview.
        #[cfg(target_os = "windows")]
        let mut context = wry::WebContext::new(
            app.path()
                .app_local_data_dir()
                .ok()
                .map(|dir| dir.join("BrowserTab")),
        );
        #[cfg(target_os = "windows")]
        let builder = WebViewBuilder::new_with_web_context(&mut context);
        #[cfg(not(target_os = "windows"))]
        let builder = WebViewBuilder::new();
        let builder = builder
            .with_url(&url)
            .with_bounds(rect(bounds))
            .with_visible(false)
            .with_incognito(true)
            .with_initialization_script_for_main_only(NO_CAPTURE, false)
            .with_initialization_script_for_main_only(FAVICON, true)
            .with_ipc_handler(move |request| {
                let url = request.uri().to_string();
                let data_url = request.into_body();
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
            .with_download_started_handler(move |url, _path| {
                let _ = on_download.send(BrowserEvent::Blocked { url });
                false
            })
            .with_on_page_load_handler(move |event, url| {
                let loading = matches!(event, PageLoadEvent::Started);
                let mut held = load_limit.lock().expect("favicon lock");
                if loading {
                    held.reset();
                }
                let _ = on_load.send(BrowserEvent::Load { url, loading });
            })
            .with_document_title_changed_handler(move |title| {
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
            other => return Err(failed(format!("unknown browser action {other}"))),
        };
        with_webview(id, |webview| webview.evaluate_script(script))
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

    pub fn close(_id: &str) {}

    pub fn close_all() {}
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
/// `events`. For a tab that has one, it goes to `url` (see `browser_navigate`).
#[tauri::command]
pub fn browser_open(
    app: tauri::AppHandle,
    id: String,
    url: String,
    bounds: BrowserBounds,
    events: Channel<BrowserEvent>,
) -> Result<(), IpcError> {
    web_page_only(&url)?;
    embedded::open(&app, id, url, bounds, events)
}

/// Shows `url` in the tab `id`'s webview, at `bounds`.
#[tauri::command]
pub fn browser_navigate(id: String, url: String, bounds: BrowserBounds) -> Result<(), IpcError> {
    web_page_only(&url)?;
    embedded::navigate(&id, &url, bounds)
}

/// Moves the tab's page to `bounds`, or hides it (`None`): the tab was hidden or covered.
#[tauri::command]
pub fn browser_place(id: String, bounds: Option<BrowserBounds>) -> Result<(), IpcError> {
    embedded::place(&id, bounds)
}

/// Back, forward, reload or stop in the tab's page.
#[tauri::command]
pub fn browser_go(id: String, action: String) -> Result<(), IpcError> {
    embedded::go(&id, &action)
}

/// Drops every tab's page (on the main thread): the app's own page started again.
pub fn close_all() {
    embedded::close_all();
}

/// The tab closed: its page and everything it stored go.
#[tauri::command]
pub fn browser_close(id: String) {
    embedded::close(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

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
