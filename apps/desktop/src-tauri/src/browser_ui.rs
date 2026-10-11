//! The Browser tab's WebKit UI delegate on macOS, and the one module in the workspace allowed
//! `unsafe` code (README, "Dependency notes").
//!
//! wry's own delegate grants every camera and microphone request a page makes. That is right
//! for the app's page, but a web page in the Browser tab must never listen or watch, so the
//! tab's webview gets this delegate instead: it denies media capture, sends popups
//! (`window.open`, `target=_blank`) to the system browser, and leaves the file picker to wry's
//! delegate as before. It also reaches the browser's saved WebKit data store (clearing it,
//! counting its sites) and asks where to save a download. Talking to WebKit through objc2 needs `unsafe`; every use says why it
//! holds.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSModalResponseOK, NSSavePanel};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSObjectProtocol, NSSet, NSString, NSURL, NSUUID,
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
            let (scheme, host) = unsafe { (origin.protocol(), origin.host()) };
            let kind = match kind {
                WKMediaCaptureType::Camera => "camera",
                WKMediaCaptureType::Microphone => "microphone",
                WKMediaCaptureType::CameraAndMicrophone => "camera and microphone",
                _ => "media",
            };
            tracing::info!(origin = %format!("{scheme}://{host}"), kind, "Browser tab: media capture denied");
            decision.call((WKPermissionDecision::Deny,));
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
    let block = RcBlock::new(move || done(true));
    // SAFETY: a live data store, WebKit's own type names and a block that outlives the call
    // (WebKit copies it), on the main thread.
    unsafe {
        data_store(identifier, mtm).removeDataOfTypes_modifiedSince_completionHandler(
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
    let block = RcBlock::new(move |records: NonNull<NSArray<WKWebsiteDataRecord>>| {
        // SAFETY: WebKit passes a valid array for the call's duration.
        done(Some(unsafe { records.as_ref() }.count()));
    });
    // SAFETY: as in `remove_data`.
    unsafe {
        data_store(identifier, mtm)
            .fetchDataRecordsOfTypes_completionHandler(&data_types(true, false), &block);
    }
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
