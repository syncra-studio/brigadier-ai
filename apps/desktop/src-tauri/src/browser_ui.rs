//! The Browser tab's WebKit UI delegate on macOS, and the one module in the workspace allowed
//! `unsafe` code (README, "Dependency notes").
//!
//! wry's own delegate grants every camera and microphone request a page makes. That is right
//! for the app's page, but a web page in the Browser tab may listen or watch only as the user
//! chose for its site, so the tab's webview gets this delegate instead: it answers media
//! capture from the site's choices (Allow, Block, or Ask: WebKit's own question), and leaves
//! popups (`window.open`, `target=_blank`; new tabs, see `browser::popup`) and the file picker to
//! wry's delegate as before. It also puts each page beneath the app's own web view (`Layer`). It also reaches the browser's saved WebKit data store (clearing it,
//! counting its sites, adding imported cookies) and asks where to save a download. A tab's page
//! also zooms, finds text, prints, copies a screenshot and reloads past the cache here. Talking
//! to WebKit through objc2 needs `unsafe`; every use says why it holds.

use std::cell::{Cell, RefCell};
use std::ffi::CStr;
use std::ptr::NonNull;

use block2::{DynBlock, RcBlock};
use brigadier_ipc::app::SitePermission;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject, Sel};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSEventMask, NSEventPhase, NSImage,
    NSModalResponseOK, NSPasteboard, NSPasteboardTypePNG, NSPrintInfo, NSSavePanel, NSView,
    NSWindowOrderingMode,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSDictionary, NSError, NSHTTPCookie, NSHTTPCookieDomain,
    NSHTTPCookieExpires, NSHTTPCookieName, NSHTTPCookiePath, NSHTTPCookiePropertyKey,
    NSHTTPCookieSameSiteLax, NSHTTPCookieSameSitePolicy, NSHTTPCookieSameSiteStrict,
    NSHTTPCookieSecure, NSHTTPCookieValue, NSMutableDictionary, NSNumber, NSObjectNSKeyValueCoding,
    NSObjectProtocol, NSPoint, NSSet, NSString, NSURL, NSUUID, ns_string,
};
use objc2_web_kit::{
    WKFindConfiguration, WKFindResult, WKFrameInfo, WKMediaCaptureType, WKNavigationAction,
    WKOpenPanelParameters, WKPermissionDecision, WKSecurityOrigin, WKUIDelegate,
    WKUserContentController, WKWebView, WKWebViewConfiguration, WKWebsiteDataRecord,
    WKWebsiteDataStore, WKWebsiteDataTypeCookies, WKWebsiteDataTypeDiskCache,
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
    /// The page closed itself.
    on_close: Box<dyn Fn()>,
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
            web_view: &WKWebView,
            configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            let create =
                sel!(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:);
            match &self.ivars().wry {
                // wry's delegate makes the popup's page, in a tab of its own (`browser::popup`).
                Some(wry) if wry.respondsToSelector(create) => {
                    // SAFETY: wry's delegate implements this method (checked just above), and
                    // it gets WebKit's own arguments unchanged.
                    unsafe {
                        wry.webView_createWebViewWithConfiguration_forNavigationAction_windowFeatures(
                            web_view, configuration, action, features,
                        )
                    }
                }
                _ => None,
            }
        }

        #[unsafe(method(webViewDidClose:))]
        fn did_close(&self, _web_view: &WKWebView) {
            (self.ivars().on_close)();
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
    /// The page, beneath the app's own web view until it goes.
    page: Retained<NSView>,
}

impl Drop for BrowserUi {
    fn drop(&mut self) {
        LAYER.with_borrow_mut(|layer| {
            layer.pages.retain(|page| *page != self.page);
            for held in [&mut layer.pressed, &mut layer.scrolling] {
                if held.as_ref() == Some(&self.page) {
                    *held = None;
                }
            }
        });
    }
}

/// A rectangle in the app's page, in CSS pixels from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Area {
    fn holds(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// How the tabs' pages and the app's own web view share the window. Each page lies beneath the
/// app's web view, which leaves a clear hole where a page shows (the app cuts its page there), so
/// the app's menus, tooltips and dialogs draw above the website while it stays live. The app's
/// web view still gets the window's clicks, scrolls and drags, and hands those in a hole to the
/// page there, unless the app has something there: one of its `above` areas (a floating
/// control), or anything `modal` (a menu, popover or dialog), which takes every click in the
/// hole, as a click beside a menu closes it.
#[derive(Default)]
struct Layer {
    /// The app's own web view, once a page has found it.
    app: Option<Retained<NSView>>,
    pages: Vec<Retained<NSView>>,
    above: Vec<Area>,
    modal: bool,
    /// The page a press began on, which has its drags and its release.
    pressed: Option<Retained<NSView>>,
    /// The page a scroll gesture began on, which has the rest of it and its momentum.
    scrolling: Option<Retained<NSView>>,
    /// The pointer was over a page when it last moved.
    over_page: bool,
}

/// Where an event the app's web view got goes.
enum Route {
    /// The app's own.
    App,
    /// The page beneath.
    Page(Retained<NSView>),
    /// Nowhere: a page beneath has it already (the pointer moving over it).
    Drop,
}

impl Layer {
    /// The page beneath where `event` happened, if it takes it there.
    fn page_at(&self, app: &NSView, event: &NSEvent) -> Option<Retained<NSView>> {
        // SAFETY: a property read on a live view, on the main thread (events arrive there).
        let parent = unsafe { app.superview() }?;
        self.page_under(
            app,
            parent.convertPoint_fromView(event.locationInWindow(), None),
        )
    }

    /// Whether a page beneath takes events at `point` (in the app web view's superview).
    fn takes(&self, app: &NSView, point: NSPoint) -> bool {
        self.page_under(app, point).is_some()
    }

    /// The page beneath at `point` (in the app web view's superview), if it takes events there.
    fn page_under(&self, app: &NSView, point: NSPoint) -> Option<Retained<NSView>> {
        if self.modal {
            return None;
        }
        // SAFETY: a property read on a live view, on the main thread.
        let parent = unsafe { app.superview() }?;
        let page = self.pages.iter().find(|page| {
            let frame = page.frame();
            !page.isHidden()
                && point.x >= frame.origin.x
                && point.x < frame.origin.x + frame.size.width
                && point.y >= frame.origin.y
                && point.y < frame.origin.y + frame.size.height
        })?;
        // The web view is flipped: its points run from the top left, as CSS pixels do.
        let local = app.convertPoint_fromView(point, Some(&parent));
        (!self.above.iter().any(|area| area.holds(local.x, local.y))).then(|| page.clone())
    }

    /// Where `event`, sent to `view` as `cmd`, goes.
    fn route(&mut self, view: &NSView, cmd: Sel, event: &NSEvent) -> Route {
        let Some(app) = self.app.clone() else {
            return Route::App;
        };
        if !std::ptr::eq(&*app, view) {
            return Route::App;
        }
        let to = |page: Option<Retained<NSView>>| page.map_or(Route::App, Route::Page);
        match cmd.name().to_bytes() {
            b"mouseDown:" | b"rightMouseDown:" | b"otherMouseDown:" => {
                self.pressed = self.page_at(&app, event);
                to(self.pressed.clone())
            }
            b"mouseDragged:" | b"rightMouseDragged:" | b"otherMouseDragged:" => {
                to(self.pressed.clone())
            }
            b"mouseUp:" | b"rightMouseUp:" | b"otherMouseUp:" => to(self.pressed.take()),
            b"scrollWheel:" => {
                let phase = event.phase();
                let fresh = phase.contains(NSEventPhase::Began)
                    || phase.contains(NSEventPhase::MayBegin)
                    || (phase.is_empty() && event.momentumPhase().is_empty());
                if fresh {
                    self.scrolling = self.page_at(&app, event);
                }
                to(self.scrolling.clone())
            }
            // The page tracks the pointer over it itself; the app hears the move into its hole
            // (so what it showed hovered there lets go), then nothing until it leaves.
            b"mouseMoved:" => {
                let over = self.page_at(&app, event).is_some();
                let entering = over && !self.over_page;
                self.over_page = over;
                if over && !entering {
                    Route::Drop
                } else {
                    Route::App
                }
            }
            _ => to(self.page_at(&app, event)),
        }
    }
}

thread_local! {
    /// The window's pages and what the app has over them. Views live on the main thread.
    static LAYER: RefCell<Layer> = RefCell::default();
    /// The web view class's own methods for the events a page beneath takes, by selector.
    static ORIGINAL: RefCell<Vec<(Sel, objc2::runtime::Imp)>> = const { RefCell::new(Vec::new()) };
    /// The web view class's own `hitTest:`.
    static HIT_TEST: Cell<Option<objc2::runtime::Imp>> = const { Cell::new(None) };
}

/// The events the app's web view hands to a page beneath.
const HANDED: [&CStr; 13] = [
    c"mouseDown:",
    c"mouseDragged:",
    c"mouseUp:",
    c"rightMouseDown:",
    c"rightMouseDragged:",
    c"rightMouseUp:",
    c"otherMouseDown:",
    c"otherMouseDragged:",
    c"otherMouseUp:",
    c"mouseMoved:",
    c"magnifyWithEvent:",
    c"smartMagnifyWithEvent:",
    c"pressureChangeWithEvent:",
];

/// What the app has over its pages now: its floating controls' areas, and whether a menu,
/// popover or dialog is open.
pub fn set_layer(above: Vec<Area>, modal: bool) {
    LAYER.with_borrow_mut(|layer| {
        layer.above = above;
        layer.modal = modal;
    });
}

/// The web view class's handler for each of the `HANDED` events: the app's web view hands one
/// in a hole to the page there; every other web view (and the app's, elsewhere) handles it as
/// before.
unsafe extern "C-unwind" fn on_event(this: &NSView, cmd: Sel, event: &NSEvent) {
    // Not while the layer is being changed.
    let route = LAYER.with(|layer| {
        layer
            .try_borrow_mut()
            .map_or(Route::App, |mut layer| layer.route(this, cmd, event))
    });
    let receiver: Retained<NSView> = match route {
        Route::App => Retained::from(this),
        Route::Page(page) => {
            if matches!(
                cmd.name().to_bytes(),
                b"mouseDown:" | b"rightMouseDown:" | b"otherMouseDown:"
            ) {
                // Typing goes to the page clicked, as a click on a page anywhere does.
                if let Some(window) = page.window() {
                    window.makeFirstResponder(Some(&page));
                }
            }
            page
        }
        Route::Drop => return,
    };
    let Some(original) = ORIGINAL.with_borrow(|original| {
        original
            .iter()
            .find(|(selector, _)| *selector == cmd)
            .map(|(_, imp)| *imp)
    }) else {
        return;
    };
    // SAFETY: `original` is the class's own method for `cmd`, an event method taking one
    // NSEvent, and `receiver` is a web view of that class (pages and the app's web view share
    // wry's class).
    unsafe {
        let original = std::mem::transmute::<
            objc2::runtime::Imp,
            unsafe extern "C-unwind" fn(&NSView, Sel, &NSEvent),
        >(original);
        original(&receiver, cmd, event);
    }
}

/// The web view class's `hitTest:`: over a page beneath that takes events there, the app's web
/// view itself (not one of WebKit's views inside it, which would take a scroll itself), so
/// `on_event` hands them on; everywhere else WebKit's own answer.
unsafe extern "C-unwind" fn hit_test(this: &NSView, cmd: Sel, point: NSPoint) -> *mut NSView {
    let Some(original) = HIT_TEST.get() else {
        return std::ptr::null_mut();
    };
    // SAFETY: `original` is the class's own `hitTest:`, which takes the same receiver and point.
    let hit = unsafe {
        std::mem::transmute::<
            objc2::runtime::Imp,
            unsafe extern "C-unwind" fn(&NSView, Sel, NSPoint) -> *mut NSView,
        >(original)(this, cmd, point)
    };
    if hit.is_null() {
        return hit;
    }
    let over_page = LAYER.with(|layer| {
        layer.try_borrow().is_ok_and(|layer| {
            layer
                .app
                .as_deref()
                .is_some_and(|app| std::ptr::eq(app, this))
                && layer.takes(this, point)
        })
    });
    if over_page {
        std::ptr::from_ref(this).cast_mut()
    } else {
        hit
    }
}

/// Scrolls over a page beneath go to it: the app sees each before any view does, and hands one
/// that begins over a page to it, with the rest of its gesture and its momentum, wherever AppKit
/// would send it.
fn hand_scrolls() {
    let block = RcBlock::new(|event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit passes a live event for the call's duration.
        let scroll = unsafe { event.as_ref() };
        let page = LAYER.with(|layer| {
            let mut layer = layer.try_borrow_mut().ok()?;
            let app = layer.app.clone()?;
            if scroll.window(app.mtm()) != app.window() {
                return None;
            }
            match layer.route(&app, sel!(scrollWheel:), scroll) {
                Route::Page(page) => Some(page),
                Route::App | Route::Drop => None,
            }
        });
        match page {
            Some(page) => {
                page.scrollWheel(scroll);
                std::ptr::null_mut()
            }
            None => event.as_ptr(),
        }
    });
    // SAFETY: a block of the signature AppKit calls, which it copies; on the main thread.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(NSEventMask::ScrollWheel, &block)
    };
    // The monitor lasts as long as the app.
    std::mem::forget(monitor);
}

/// Puts `page` beneath the app's own web view in their window, and on the first page makes the
/// app's web view clear where its page is clear, and hand events in a page's hole to the page.
fn beneath_app(page: &WKWebView) {
    // SAFETY: a property read on a live view, on the main thread.
    let Some(parent) = (unsafe { page.superview() }) else {
        return;
    };
    let page: &NSView = page;
    let page = Retained::from(page);
    LAYER.with_borrow_mut(|layer| {
        if layer.app.is_none() {
            // The window's first web view that is no page: the app's own.
            let web_view = WKWebView::class();
            let app = parent.subviews().iter().find(|view| {
                *view != page && view.isKindOfClass(web_view) && !layer.pages.contains(view)
            });
            let Some(app) = app else {
                tracing::warn!("Browser tab: the app's web view isn't in the window");
                return;
            };
            if !clear_app(&app) {
                return;
            }
            layer.app = Some(app);
        }
        let Some(app) = layer.app.as_deref() else {
            return;
        };
        parent.addSubview_positioned_relativeTo(&page, NSWindowOrderingMode::Below, Some(app));
        layer.pages.push(page);
    });
}

/// Makes the app's web view draw nothing where its page is clear, and gives its class the
/// `HANDED` events' handler. `false` when the class takes no handler.
fn clear_app(app: &NSView) -> bool {
    // NOTE: `drawsBackground` is WebKit's private key, as wry and Tauri use it for a clear page.
    // SAFETY: a key-value write the web view accepts (wry writes the same key), main thread.
    unsafe {
        app.setValue_forKey(
            Some(&NSNumber::new_bool(false)),
            ns_string!("drawsBackground"),
        )
    };
    // The web view's own class (not a key-value observing subclass, which is made later and
    // inherits what this one has): the first in its chain WebKit didn't define.
    let mut class = app.class();
    while let Some(superclass) = class.superclass() {
        if std::ptr::eq(superclass, WKWebView::class()) {
            break;
        }
        class = superclass;
    }
    let mut original = Vec::new();
    for name in HANDED {
        let selector = Sel::register(name);
        // SAFETY: NSResponder declares every `HANDED` event method, taking one NSEvent and
        // returning nothing, so the class answers each with that signature and its type
        // encoding; the handler has that signature and calls the method it replaces.
        unsafe {
            let method = objc2::ffi::class_getInstanceMethod(class, selector);
            if method.is_null() {
                return false;
            }
            let imp = objc2::ffi::method_getImplementation(method);
            let Some(imp) = imp else {
                return false;
            };
            original.push((selector, imp));
            let types = objc2::ffi::method_getTypeEncoding(method);
            let replacement = std::mem::transmute::<
                unsafe extern "C-unwind" fn(&NSView, Sel, &NSEvent),
                objc2::runtime::Imp,
            >(on_event);
            objc2::ffi::class_replaceMethod(
                std::ptr::from_ref(class).cast_mut(),
                selector,
                replacement,
                types,
            );
        }
    }
    ORIGINAL.with_borrow_mut(|o| *o = original);
    hand_scrolls();
    let selector = sel!(hitTest:);
    // SAFETY: NSView declares `hitTest:`, taking a point and returning a view; `hit_test` has
    // that signature and calls the method it replaces.
    unsafe {
        let method = objc2::ffi::class_getInstanceMethod(class, selector);
        let Some(imp) = (!method.is_null())
            .then(|| objc2::ffi::method_getImplementation(method))
            .flatten()
        else {
            return false;
        };
        HIT_TEST.set(Some(imp));
        let types = objc2::ffi::method_getTypeEncoding(method);
        let replacement = std::mem::transmute::<
            unsafe extern "C-unwind" fn(&NSView, Sel, NSPoint) -> *mut NSView,
            objc2::runtime::Imp,
        >(hit_test);
        objc2::ffi::class_replaceMethod(
            std::ptr::from_ref(class).cast_mut(),
            selector,
            replacement,
            types,
        );
    }
    true
}

/// Makes `webview` answer camera and microphone requests from the site's choices, tell
/// `on_close` when its page closes itself, and lie beneath the app's web view. Call it on the
/// main thread, right after building the webview.
pub fn install(webview: &wry::WebView, on_close: impl Fn() + 'static) -> BrowserUi {
    let view = webview.webview();
    // SAFETY: a property read on a live web view, on the main thread (it is MainThreadOnly).
    let wry = unsafe { view.UIDelegate() };
    let delegate = Delegate::new(
        view.mtm(),
        Ivars {
            wry,
            on_close: Box::new(on_close),
        },
    );
    // SAFETY: the delegate implements WKUIDelegate; WebKit keeps it weakly, and the returned
    // `BrowserUi` keeps it alive beside the webview.
    unsafe { view.setUIDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
    beneath_app(&view);
    let page: &NSView = &view;
    BrowserUi {
        _delegate: delegate,
        page: Retained::from(page),
    }
}

/// Gives a popup's `configuration` (a copy of its opener's) scripts and messages of its own:
/// shared, wry would add the page's own to the opener's, where they'd run twice and clash.
pub fn own_scripts(configuration: &WKWebViewConfiguration) {
    // SAFETY: a fresh controller set on the configuration before any web view is made with it,
    // on the main thread (the configuration is MainThreadOnly).
    unsafe {
        configuration.setUserContentController(&WKUserContentController::new(configuration.mtm()));
    }
}

/// The page's zoom, 1.0 being 100%.
pub fn page_zoom(webview: &wry::WebView) -> f64 {
    // SAFETY: a property read on a live web view, on the main thread (it is MainThreadOnly).
    unsafe { webview.webview().pageZoom() }
}

/// Zooms the page to `zoom` (1.0 being 100%). It stays with the web view across its pages.
pub fn set_page_zoom(webview: &wry::WebView, zoom: f64) {
    // SAFETY: a property write on a live web view, on the main thread.
    unsafe { webview.webview().setPageZoom(zoom) };
}

/// Finds `query` in the page, after the current match (before it, `backwards`), going round at
/// the end and ignoring case. WebKit selects the match and scrolls to it; `done` hears whether
/// there was one.
pub fn find(
    webview: &wry::WebView,
    query: &str,
    backwards: bool,
    done: impl FnOnce(bool) + 'static,
) {
    let view = webview.webview();
    // SAFETY: a fresh configuration and plain property writes, on the main thread.
    let configuration = unsafe {
        let configuration = WKFindConfiguration::new(view.mtm());
        configuration.setBackwards(backwards);
        configuration.setCaseSensitive(false);
        configuration.setWraps(true);
        configuration
    };
    let done = once(done);
    let block = RcBlock::new(move |result: NonNull<WKFindResult>| {
        // SAFETY: WebKit passes a valid result for the call's duration.
        done(unsafe { result.as_ref().matchFound() });
    });
    // SAFETY: a live web view, a valid string and configuration, and a block WebKit copies, on
    // the main thread.
    unsafe {
        view.findString_withConfiguration_completionHandler(
            &NSString::from_str(query),
            Some(&configuration),
            &block,
        );
    }
}

/// Opens the print panel for the page, as a sheet on its window. `false` when the page is in
/// no window.
pub fn print(webview: &wry::WebView) -> bool {
    let view = webview.webview();
    let Some(window) = view.window() else {
        return false;
    };
    let info = NSPrintInfo::sharedPrintInfo();
    // SAFETY: a live web view and the shared print info, on the main thread.
    let operation = unsafe { view.printOperationWithPrintInfo(&info) };
    // WebKit's printing view starts out with no size, and prints blank pages unless it is given
    // the page's.
    if let Some(printing) = operation.view() {
        printing.setFrame(view.bounds());
    }
    operation.setShowsPrintPanel(true);
    operation.setShowsProgressPanel(true);
    // SAFETY: the page's own window; no delegate, so no selector or context is called back.
    unsafe {
        operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            &window,
            None,
            None,
            std::ptr::null_mut(),
        );
    }
    true
}

/// `image` as PNG on the general pasteboard, in place of what was there.
fn copy_png(image: &NSImage) -> bool {
    let Some(rep) = image
        .TIFFRepresentation()
        .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
    else {
        return false;
    };
    // SAFETY: an empty properties dictionary, which the method accepts for every file type.
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    };
    let Some(png) = png else {
        return false;
    };
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: AppKit's exported pasteboard type, read only.
    pasteboard.setData_forType(Some(&png), unsafe { NSPasteboardTypePNG })
}

/// Takes a picture of the page as shown and puts it on the clipboard as a PNG; `done` hears
/// whether it is there.
pub fn copy_snapshot(webview: &wry::WebView, done: impl FnOnce(bool) + 'static) {
    let done = once(done);
    let block = RcBlock::new(move |image: *mut NSImage, _error: *mut NSError| {
        // SAFETY: WebKit passes either nil or a valid image for the call's duration.
        let copied = unsafe { image.as_ref() }.is_some_and(copy_png);
        done(copied);
    });
    // SAFETY: a live web view, no configuration (the whole visible page) and a block WebKit
    // copies, on the main thread.
    unsafe {
        webview
            .webview()
            .takeSnapshotWithConfiguration_completionHandler(None, &block);
    }
}

/// Reloads the page, fetching it and what it loads again rather than from the cache.
pub fn reload_from_origin(webview: &wry::WebView) {
    // SAFETY: a plain method call on a live web view, on the main thread.
    let _ = unsafe { webview.webview().reloadFromOrigin() };
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

/// How long an imported session cookie lasts: 400 days, the longest any cookie may live in
/// Chromium.
const SESSION_COOKIE_LIFETIME_SECS: u64 = 400 * 24 * 60 * 60;

/// `cookie` as WebKit takes it, built from its properties much as wry 0.55.1 does
/// (`cookie_into_wkwebview`). "HttpOnly" has no Foundation key constant, so it goes in by name,
/// and only when true (wry also sets it to "FALSE", which Foundation reads as HttpOnly);
/// SameSite uses `NSHTTPCookieSameSitePolicy`. A domain with a leading dot is sent to its
/// subdomains too; without one only to that host. A session cookie gets an expiry
/// [`SESSION_COOKIE_LIFETIME_SECS`] away: WebKit keeps one without an expiry in memory only, so
/// it would be gone at the next launch, while the browser it came from restores it with the
/// session and keeps the sign-in. `None` when Foundation refuses the properties.
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
            None => {
                let expires =
                    NSDate::dateWithTimeIntervalSinceNow(SESSION_COOKIE_LIFETIME_SECS as f64);
                properties.insert(NSHTTPCookieExpires, &*expires);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(expires_unix: Option<i64>) -> brigadier_browser_data::Cookie {
        brigadier_browser_data::Cookie {
            name: "sid".into(),
            value: "1".into(),
            domain: ".example.com".into(),
            host_only: false,
            path: "/".into(),
            expires_unix,
            secure: true,
            http_only: true,
            same_site: None,
        }
    }

    #[test]
    fn a_session_cookie_is_kept_past_a_relaunch() {
        let built = ns_cookie(&cookie(None)).expect("Foundation takes the cookie");
        let (session_only, expires) = (built.isSessionOnly(), built.expiresDate());
        assert!(!session_only);
        let left = expires.expect("an expiry").timeIntervalSinceNow();
        let lifetime = SESSION_COOKIE_LIFETIME_SECS as f64;
        assert!(left > lifetime - 60.0 && left <= lifetime, "{left}");
    }

    #[test]
    fn a_cookie_keeps_its_own_expiry() {
        // Within Foundation's own 400-day cap, which shortens any later expiry.
        let at = NSDate::now().timeIntervalSince1970().floor() as i64 + 30 * 24 * 60 * 60;
        let built = ns_cookie(&cookie(Some(at))).expect("Foundation takes the cookie");
        let expires = built.expiresDate().expect("an expiry");
        assert_eq!(expires.timeIntervalSince1970(), at as f64);
    }
}
