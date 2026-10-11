//! The Browser tab's WebKit UI delegate on macOS, and the one module in the workspace allowed
//! `unsafe` code (README, "Dependency notes").
//!
//! wry's own delegate grants every camera and microphone request a page makes. That is right
//! for the app's page, but a web page in the Browser tab may listen or watch only as the user
//! chose for its site, so the tab's webview gets this delegate instead: it answers media
//! capture from the site's choices (Allow, Block, or Ask: WebKit's own question), sends popups
//! (`window.open`, `target=_blank`) to the system browser, and leaves the file picker to wry's
//! delegate as before. It also reaches the browser's saved WebKit data store (clearing it,
//! counting its sites, adding imported cookies) and asks where to save a download. Talking to
//! WebKit through objc2 needs `unsafe`; every use says why it holds.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::{DynBlock, RcBlock};
use brigadier_ipc::app::SitePermission;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSHTTPCookie, NSHTTPCookieDiscard, NSHTTPCookieDomain,
    NSHTTPCookieExpires, NSHTTPCookieName, NSHTTPCookiePath, NSHTTPCookiePropertyKey,
    NSHTTPCookieSameSiteLax, NSHTTPCookieSameSitePolicy, NSHTTPCookieSameSiteStrict,
    NSHTTPCookieSecure, NSHTTPCookieValue, NSMutableDictionary, NSObjectProtocol, NSSet, NSString,
    NSURL, NSUUID, ns_string,
};
use objc2_web_kit::{
    WKFrameInfo, WKMediaCaptureType, WKNavigationAction, WKOpenPanelParameters,
    WKPermissionDecision, WKSecurityOrigin, WKUIDelegate, WKWebView, WKWebViewConfiguration,
    WKWebsiteDataRecord, WKWebsiteDataStore, WKWebsiteDataTypeCookies, WKWebsiteDataTypeDiskCache,
    WKWebsiteDataTypeFetchCache, WKWebsiteDataTypeFileSystem, WKWebsiteDataTypeHashSalt,
    WKWebsiteDataTypeIndexedDBDatabases, WKWebsiteDataTypeLocalStorage, WKWebsiteDataTypeMediaKeys,
    WKWebsiteDataTypeMemoryCache, WKWebsiteDataTypeSearchFieldRecentSearches,
    WKWebsiteDataTypeServiceWorkerRegistrations, WKWebsiteDataTypeSessionStorage,
    WKWebsiteDataTypeWebSQLDatabases, WKWindowFeatures,
};
use wry::WebViewExtMacOS;

use crate::browser_store::{self, Device};

struct Ivars {
    /// wry's delegate, which still shows the file picker.
    wry: Option<Retained<ProtocolObject<dyn WKUIDelegate>>>,
    /// Opens a popup's address outside the tab.
    on_popup: Box<dyn Fn(String)>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class adds no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    #[name = "BrigadierBrowserUIDelegate"]
    struct Delegate;

    // SAFETY: NSObjectProtocol has no requirements.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: each method's selector and signature match WKUIDelegate's declaration in
    // objc2-web-kit.
    unsafe impl WKUIDelegate for Delegate {
        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn request_media_capture(
            &self,
            _web_view: &WKWebView,
            origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            kind: WKMediaCaptureType,
            decision: &DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            // SAFETY: plain property reads on the origin WebKit passed in.
            let (scheme, host, port) = unsafe { (origin.protocol(), origin.host(), origin.port()) };
            let origin = if port > 0 {
                format!("{scheme}://{host}:{port}")
            } else {
                format!("{scheme}://{host}")
            };
            let devices: &[Device] = match kind {
                WKMediaCaptureType::Camera => &[Device::Camera],
                WKMediaCaptureType::Microphone => &[Device::Microphone],
                WKMediaCaptureType::CameraAndMicrophone => &[Device::Camera, Device::Microphone],
                _ => &[],
            };
            // Every site that asks is listed in Site settings, with the user's choices.
            let now = browser_store::now_ms();
            let choices: Option<Vec<SitePermission>> = devices
                .iter()
                .map(|device| browser_store::with(|store| store.site_asked(&origin, *device, now)))
                .collect();
            let answer = match choices.map(|choices| browser_store::combined(&choices)) {
                Some(SitePermission::Allow) => WKPermissionDecision::Grant,
                // WebKit's own question, naming the site.
                Some(SitePermission::Ask) => WKPermissionDecision::Prompt,
                _ => WKPermissionDecision::Deny,
            };
            tracing::info!(%origin, ?devices, ?answer, "Browser tab: camera or microphone asked for");
            decision.call((answer,));
        }

        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            _web_view: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            // SAFETY: a property read on the action WebKit passed in.
            let request = unsafe { action.request() };
            if let Some(url) = request.URL().and_then(|url| url.absoluteString()) {
                (self.ivars().on_popup)(url.to_string());
            }
            // No new web view: WebKit opens nothing itself.
            None
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn run_open_panel(
            &self,
            web_view: &WKWebView,
            parameters: &WKOpenPanelParameters,
            frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
        ) {
            let open_panel =
                sel!(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:);
            match &self.ivars().wry {
                Some(wry) if wry.respondsToSelector(open_panel) => {
                    // SAFETY: wry's delegate implements this method (checked just above), and
                    // it gets WebKit's own arguments unchanged.
                    unsafe {
                        wry.webView_runOpenPanelWithParameters_initiatedByFrame_completionHandler(
                            web_view, parameters, frame, done,
                        );
                    }
                }
                // As if the user cancelled the picker.
                _ => done.call((std::ptr::null_mut(),)),
            }
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, ivars: Ivars) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(ivars);
        // SAFETY: NSObject's `init` on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}

/// The Browser tab's delegate for one webview. WebKit holds a delegate weakly, so this must
/// live as long as the webview.
pub struct BrowserUi {
    _delegate: Retained<Delegate>,
}

/// Makes `webview` deny camera and microphone requests and hand popups' addresses to
/// `on_popup` rather than open them. Call it on the main thread, right after building the
/// webview.
pub fn install(webview: &wry::WebView, on_popup: impl Fn(String) + 'static) -> BrowserUi {
    let view = webview.webview();
    // SAFETY: a property read on a live web view, on the main thread (it is MainThreadOnly).
    let wry = unsafe { view.UIDelegate() };
    let delegate = Delegate::new(
        view.mtm(),
        Ivars {
            wry,
            on_popup: Box::new(on_popup),
        },
    );
    // SAFETY: the delegate implements WKUIDelegate; WebKit keeps it weakly, and the returned
    // `BrowserUi` keeps it alive beside the webview.
    unsafe { view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    BrowserUi {
        _delegate: delegate,
    }
}

/// The browser's saved data store, by the identifier its webviews were built with.
fn data_store(identifier: [u8; 16], mtm: MainThreadMarker) -> Retained<WKWebsiteDataStore> {
    // SAFETY: a class method taking a non-zero UUID (the identifier is a hash, never all zero
    // in practice; WebKit throws only for the nil UUID), on the main thread.
    unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&NSUUID::from_bytes(identifier), mtm) }
}

/// WebKit's names for what a site keeps (`site_data`) and for its caches (`cache`). Every one is
/// in macOS 14, the app's oldest.
fn data_types(site_data: bool, cache: bool) -> Retained<NSSet<NSString>> {
    // SAFETY: WebKit's exported constant strings, read only.
    let names: Vec<&NSString> = unsafe {
        let mut names = Vec::new();
        if site_data {
            names.extend([
                WKWebsiteDataTypeCookies,
                WKWebsiteDataTypeLocalStorage,
                WKWebsiteDataTypeSessionStorage,
                WKWebsiteDataTypeIndexedDBDatabases,
                WKWebsiteDataTypeWebSQLDatabases,
                WKWebsiteDataTypeServiceWorkerRegistrations,
                WKWebsiteDataTypeFileSystem,
                WKWebsiteDataTypeSearchFieldRecentSearches,
                WKWebsiteDataTypeMediaKeys,
                WKWebsiteDataTypeHashSalt,
            ]);
        }
        if cache {
            names.extend([
                WKWebsiteDataTypeDiskCache,
                WKWebsiteDataTypeMemoryCache,
                WKWebsiteDataTypeFetchCache,
            ]);
        }
        names
    };
    NSSet::from_slice(&names)
}

/// A WebKit completion block that runs `done` once.
fn once<A>(done: impl FnOnce(A) + 'static) -> impl Fn(A) + 'static {
    let done = RefCell::new(Some(done));
    move |value| {
        if let Some(done) = done.borrow_mut().take() {
            done(value);
        }
    }
}

/// Removes the browser's site data (cookies, storage, databases, service workers) and/or its
/// caches changed since `since_ms` (all of it with none), then calls `done`. On the main thread
/// only: `done` gets `false` anywhere else.
pub fn remove_data(
    identifier: [u8; 16],
    site_data: bool,
    cache: bool,
    since_ms: Option<i64>,
    done: impl FnOnce(bool) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        done(false);
        return;
    };
    let since = since_ms.map_or(0.0, |ms| ms as f64 / 1000.0);
    let date = NSDate::dateWithTimeIntervalSince1970(since);
    let done = once(done);
    let store = data_store(identifier, mtm);
    // The block holds the store until WebKit is done: an operation of a store that is gone
    // ends early, with nothing done.
    let kept = store.clone();
    let block = RcBlock::new(move || {
        let _ = &kept;
        done(true);
    });
    // SAFETY: a live data store, WebKit's own type names and a block that outlives the call
    // (WebKit copies it), on the main thread.
    unsafe {
        store.removeDataOfTypes_modifiedSince_completionHandler(
            &data_types(site_data, cache),
            &date,
            &block,
        );
    }
}

/// Counts the sites (domains) the browser keeps cookies or other site data for, then calls
/// `done` with it (`None` off the main thread).
pub fn count_sites(identifier: [u8; 16], done: impl FnOnce(Option<usize>) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else {
        done(None);
        return;
    };
    let done = once(done);
    let store = data_store(identifier, mtm);
    // Held until WebKit answers, as in `remove_data`.
    let kept = store.clone();
    let block = RcBlock::new(move |records: NonNull<NSArray<WKWebsiteDataRecord>>| {
        let _ = &kept;
        // SAFETY: WebKit passes a valid array for the call's duration.
        done(Some(unsafe { records.as_ref() }.count()));
    });
    // SAFETY: as in `remove_data`.
    unsafe { store.fetchDataRecordsOfTypes_completionHandler(&data_types(true, false), &block) };
}

/// Asks where to save a download, starting at `suggested` (its folder and name): the path
/// chosen, or `None` when the user cancels. Modal; on the main thread only (`None` elsewhere).
pub fn save_path(suggested: &std::path::Path) -> Option<std::path::PathBuf> {
    let mtm = MainThreadMarker::new()?;
    let panel = NSSavePanel::savePanel(mtm);
    panel.setCanCreateDirectories(true);
    if let Some(dir) = suggested.parent() {
        panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(
            &dir.to_string_lossy(),
        ))));
    }
    if let Some(name) = suggested.file_name() {
        panel.setNameFieldStringValue(&NSString::from_str(&name.to_string_lossy()));
    }
    if panel.runModal() != NSModalResponseOK {
        return None;
    }
    let path = panel.URL()?.path()?;
    Some(std::path::PathBuf::from(path.to_string()))
}

/// `cookie` as WebKit takes it, built from its properties much as wry 0.55.1 does
/// (`cookie_into_wkwebview`). "HttpOnly" has no Foundation key constant, so it goes in by name,
/// and only when true (wry also sets it to "FALSE", which Foundation reads as HttpOnly);
/// SameSite uses `NSHTTPCookieSameSitePolicy`. A domain with a leading dot is sent to its
/// subdomains too; without one only to that host. `None` when Foundation refuses the
/// properties.
fn ns_cookie(cookie: &brigadier_browser_data::Cookie) -> Option<Retained<NSHTTPCookie>> {
    use brigadier_browser_data::SameSite;
    let truth = |on: bool| {
        if on {
            ns_string!("TRUE")
        } else {
            ns_string!("FALSE")
        }
    };
    let (name, value, domain, path) = (
        NSString::from_str(&cookie.name),
        NSString::from_str(&cookie.value),
        NSString::from_str(&cookie.domain),
        NSString::from_str(if cookie.path.is_empty() {
            "/"
        } else {
            &cookie.path
        }),
    );
    // SAFETY: Foundation's exported constant keys, read only.
    let properties = unsafe {
        let properties: Retained<NSMutableDictionary<NSHTTPCookiePropertyKey, AnyObject>> =
            NSMutableDictionary::from_slices(
                &[
                    NSHTTPCookieName,
                    NSHTTPCookieValue,
                    NSHTTPCookieDomain,
                    NSHTTPCookiePath,
                ],
                &[&name, &value, &domain, &path],
            );
        match cookie.expires_unix {
            Some(at) => {
                let expires = NSDate::dateWithTimeIntervalSince1970(at as f64);
                properties.insert(NSHTTPCookieExpires, &*expires);
            }
            None => properties.insert(NSHTTPCookieDiscard, truth(true)),
        }
        if cookie.secure {
            properties.insert(NSHTTPCookieSecure, truth(true));
        }
        properties
    };
    // Foundation reads a cookie as HttpOnly when the key is there at all, whatever its value,
    // so it is set only for one that is.
    if cookie.http_only {
        properties.insert(ns_string!("HttpOnly"), truth(true));
    }
    // SAFETY: Foundation's exported constants (macOS 10.15 and later), read only.
    unsafe {
        match cookie.same_site {
            Some(SameSite::Lax) => {
                properties.insert(NSHTTPCookieSameSitePolicy, NSHTTPCookieSameSiteLax);
            }
            Some(SameSite::Strict) => {
                properties.insert(NSHTTPCookieSameSitePolicy, NSHTTPCookieSameSiteStrict);
            }
            _ => {}
        }
    }
    // SAFETY: a dictionary of Foundation's cookie properties, as the method expects.
    unsafe { NSHTTPCookie::cookieWithProperties(&properties) }
}

/// What a cookie is known by in the store, and the attributes it must keep.
fn identity(
    name: &str,
    domain: &str,
    path: &str,
    value: &str,
    http_only: bool,
    secure: bool,
) -> (String, String, String, String, bool, bool) {
    let path = if path.is_empty() { "/" } else { path };
    (
        name.to_owned(),
        domain.to_ascii_lowercase(),
        path.to_owned(),
        value.to_owned(),
        http_only,
        secure,
    )
}

/// Adds `cookies` to the browser's data store with `WKHTTPCookieStore setCookie`, reporting each
/// one WebKit has stored through `progress`, then reads the store back: `done` gets, per cookie,
/// whether it is there with its name, domain, path, value, HttpOnly and Secure (setCookie's
/// completion says nothing about success). On the main thread only (`done` gets nothing
/// elsewhere).
pub fn set_cookies(
    identifier: [u8; 16],
    cookies: Vec<brigadier_browser_data::Cookie>,
    progress: impl Fn(usize) + 'static,
    done: impl FnOnce(Vec<bool>) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        done(Vec::new());
        return;
    };
    // The cookie store points only weakly at its data store, and ends its work early once
    // that is gone: the data store is held until the cookies are read back.
    let data = data_store(identifier, mtm);
    // SAFETY: a property read on a live data store, on the main thread.
    let store = unsafe { data.httpCookieStore() };
    let built: Vec<Option<Retained<NSHTTPCookie>>> = cookies.iter().map(ns_cookie).collect();
    let total = built.iter().flatten().count();
    let check = {
        let store = store.clone();
        let done = once(done);
        move || {
            let block = RcBlock::new(move |found: NonNull<NSArray<NSHTTPCookie>>| {
                let _ = &data;
                // SAFETY: WebKit passes a valid array for the call's duration.
                let found = unsafe { found.as_ref() };
                let stored: std::collections::HashSet<_> = found
                    .iter()
                    .map(|cookie| {
                        identity(
                            &cookie.name().to_string(),
                            &cookie.domain().to_string(),
                            &cookie.path().to_string(),
                            &cookie.value().to_string(),
                            cookie.isHTTPOnly(),
                            cookie.isSecure(),
                        )
                    })
                    .collect();
                done(
                    cookies
                        .iter()
                        .map(|cookie| {
                            stored.contains(&identity(
                                &cookie.name,
                                &cookie.domain,
                                &cookie.path,
                                &cookie.value,
                                cookie.http_only,
                                cookie.secure,
                            ))
                        })
                        .collect(),
                );
            });
            // SAFETY: a live cookie store and a block WebKit copies, on the main thread.
            unsafe { store.getAllCookies(&block) };
        }
    };
    if total == 0 {
        check();
        return;
    }
    let state = std::rc::Rc::new(RefCell::new((0_usize, Some(check))));
    let progress = std::rc::Rc::new(progress);
    for cookie in built.iter().flatten() {
        let (state, progress) = (state.clone(), progress.clone());
        let block = RcBlock::new(move || {
            let finished = {
                let mut state = state.borrow_mut();
                state.0 += 1;
                progress(state.0);
                if state.0 == total {
                    state.1.take()
                } else {
                    None
                }
            };
            if let Some(check) = finished {
                check();
            }
        });
        // SAFETY: a live cookie store, a valid cookie and a block WebKit copies, on the main
        // thread.
        unsafe { store.setCookie_completionHandler(cookie, Some(&block)) };
    }
}
