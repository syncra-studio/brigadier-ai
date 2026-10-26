//! The visual frame's native boundary. macOS installs a content rule before serving the
//! shell, including connection hints that CSP does not block. The small unsafe section calls
//! WebKit on its main thread; the controller retains the compiled rule list.
use std::sync::Mutex;

type Waiting = Box<dyn FnOnce(bool) + Send>;
struct Frames {
    ready: Option<bool>,
    waiting: Vec<Waiting>,
}
static FRAMES: Mutex<Frames> = Mutex::new(Frames {
    ready: if cfg!(target_os = "macos") {
        None
    } else {
        Some(true)
    },
    waiting: Vec::new(),
});

fn lockdown_script() -> &'static str {
    include_str!("visuals_frame.html")
        .split_once("<script>")
        .expect("frame lockdown script")
        .1
        .split_once("</script>")
        .expect("frame lockdown end")
        .0
}

/// Native document-start scripts also cover srcdoc, whose inline scripts WebKit permits
/// despite frame-src. Replace those child documents before their markup can execute.
pub fn initialization_script() -> String {
    format!(
        r#"if (window.parent !== window) {{
        {}
        if (location.protocol === 'about:') {{
            document.open();
            document.write('<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'none\'; frame-src \'none\'; form-action \'none\'">');
            document.close();
        }}
    }}"#,
        lockdown_script()
    )
}

/// A frame cannot execute while the native blocker is still being compiled.
pub fn when_ready(reply: impl FnOnce(bool) + Send + 'static) {
    let mut frames = FRAMES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(ready) = frames.ready {
        drop(frames);
        reply(ready);
    } else {
        frames.waiting.push(Box::new(reply));
    }
}

#[cfg(target_os = "macos")]
fn complete(ready: bool) {
    let waiting = {
        let mut frames = FRAMES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        frames.ready = Some(ready);
        std::mem::take(&mut frames.waiting)
    };
    for reply in waiting {
        reply(ready);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &tauri::AppHandle) {}

#[cfg(target_os = "macos")]
pub fn install(app: &tauri::AppHandle) {
    use block2::RcBlock;
    use objc2::MainThreadMarker;
    use objc2_foundation::{NSError, NSString};
    use objc2_web_kit::{WKContentRuleList, WKContentRuleListStore, WKWebView};
    use tauri::Manager;

    let Some(window) = app.get_webview_window(crate::shell::MAIN_WINDOW) else {
        complete(false);
        return;
    };
    if let Err(error) = window.with_webview(|view| {
        let Some(mtm) = MainThreadMarker::new() else { complete(false); return; };
        // SAFETY: with_webview runs on the UI thread. Its pointer is the live main WKWebView.
        unsafe {
            let webview = &*view.inner().cast::<WKWebView>();
            let controller = webview.configuration().userContentController();
            let Some(store) = WKContentRuleListStore::defaultStore(mtm) else { complete(false); return; };
            let callback = RcBlock::new(move |list: *mut WKContentRuleList, error: *mut NSError| {
                if list.is_null() || !error.is_null() {
                    tracing::error!("could not compile the visual frame's network block");
                    complete(false);
                    return;
                }
                controller.addContentRuleList(&*list);
                complete(true);
            });
            store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                Some(&NSString::from_str("brigadier-visuals-offline")),
                Some(&NSString::from_str(r#"[{"trigger":{"url-filter":"^[a-z]+://.*","if-frame-url":["^brigadier-ui:.*","^about:.*"]},"action":{"type":"block"}}]"#)),
                Some(&callback),
            );
        }
    }) {
        tracing::error!(%error, "could not install the visual frame's network block");
        complete(false);
    }
}
