//! The side panel's Browser tab: a system webview laid over the tab's area of the window.
//!
//! It is a plain `wry` webview, not a Tauri one: Tauri gives each of its webviews the app's IPC
//! bridge, init scripts and custom protocols, and a web page must get none of that. This one
//! has no IPC handler and no custom protocols, and its one script of ours only takes the
//! microphone, camera and speech APIs away (`NO_CAPTURE`); it keeps its cookies and
//! storage in memory only, separate from the app's own webview; it opens only web pages, sends
//! popups to the system browser, and on macOS denies the page the camera and microphone (wry's
//! own delegate would grant them; see `browser_ui`). It is made when the tab first loads a page and dropped
//! when the tab closes. Linux has no embedded page (Tauri's window there is a GTK box a child
//! can't be laid over); its tab opens pages in the system browser.

use brigadier_ipc::app::{BrowserBounds, BrowserEvent};
use brigadier_ipc::protocol::{ErrorCode, IpcError};
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

    use brigadier_ipc::app::{BrowserBounds, BrowserEvent};
    use brigadier_ipc::protocol::IpcError;
    use tauri::ipc::Channel;
    use tauri::{AppHandle, Manager};
    use tauri_plugin_opener::OpenerExt;
    #[cfg(target_os = "windows")]
    use wry::NewWindowResponse;
    use wry::dpi::{LogicalPosition, LogicalSize};
    use wry::{PageLoadEvent, Rect, WebView, WebViewBuilder};

    use super::{NO_CAPTURE, allowed, failed, web_page};
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
        let (on_load, on_title, on_blocked, on_download) =
            (events.clone(), events.clone(), events.clone(), events);
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
            .with_incognito(true)
            .with_initialization_script_for_main_only(NO_CAPTURE, false)
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
                .and_then(|()| webview.set_visible(true))
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
