//! The first launch's intro, from the window's side (the page plays the rest: app/onboarding).
//!
//! On macOS the first launch holds the main window clear until the page is ready (shown, so
//! its page runs and paints, but unseen and passing the mouse through), then stages it: the
//! window appears with no title-bar buttons, shadow or background (so only the page's comet
//! is seen), above a borderless black window over the screen below the menu bar that ignores
//! the mouse and fades in to dim the desktop. When the mark's burst has opened the window, its
//! chrome comes back (`intro_shaped`); then the dim fades out and closes for good
//! (`intro_finish`).
//!
//! The page drives those beats, but never has to clean up: hiding or closing the window, a
//! page load and a deadline after `intro_begin` all end the intro here, and every end restores
//! the window. Elsewhere, or when the window was never held, `intro_begin` answers `false` and
//! the page plays the intro inside the normal window.
//!
//! Window ordering and the dim's fade go through AppKit (objc2), which needs `unsafe`; this
//! module is one of the shell's narrow exceptions (README, "Dependency notes") and each use
//! says why it holds.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

use crate::shell::{self, MAIN_WINDOW};

/// The main window was held clear at launch for the intro and hasn't been shown yet.
static HELD: AtomicBool = AtomicBool::new(false);
/// The window is staged (from `intro_begin` until the intro ends).
static STAGED: AtomicBool = AtomicBool::new(false);
/// Counts the intro's ends: a deadline only ends the staging it was armed for.
static ENDS: AtomicU64 = AtomicU64::new(0);

/// How long a held window waits for the page to begin the intro before it shows anyway.
const HOLD_DEADLINE: Duration = Duration::from_secs(15);
/// The longest the window stays staged, whatever the page does.
const STAGED_DEADLINE: Duration = Duration::from_secs(20);
/// The dim's fade when the intro is ended early (Escape, a deadline), in seconds.
const QUICK_FADE: f64 = 0.3;
/// Sent to the page when the intro ends here, so it shows its end at once.
const ENDED_EVENT: &str = "intro-ended";

/// Whether this launch holds the main window for the intro: the first launch on macOS, when
/// the window opens for the user (not under `--smoke`, not to host a notification).
pub fn should_hold(first_launch: bool, smoke: bool, opened_for_notification: bool) -> bool {
    cfg!(target_os = "macos") && first_launch && !smoke && !opened_for_notification
}

/// Holds the main window clear from launch (the caller hides it in the config, so it never
/// shows before this), and shows it normally if the page hasn't begun the intro in time. A
/// hidden window's page doesn't paint, and startup waits for a paint, so the held window is
/// shown, only clear. Call on the main thread (setup).
pub fn hold(app: &AppHandle) {
    if !mac::hold(app) {
        shell::show_main(app);
        return;
    }
    HELD.store(true, Ordering::Release);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(HOLD_DEADLINE);
        if HELD.swap(false, Ordering::AcqRel) {
            tracing::warn!("the page didn't begin the intro; showing the window");
            unhold(&app);
        }
    });
}

/// Shows a held window as it is.
fn unhold(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || mac::unhold(&handle));
}

/// Whether the window is staged: its startup colour must not cover the page's comet.
pub fn staged() -> bool {
    STAGED.load(Ordering::Acquire)
}

/// Stages the held window for the intro and shows it; `false` when it wasn't held (another
/// platform, a later launch, or the window already shown), so the page plays the intro inside
/// the normal window. Answers once the window is on screen.
#[tauri::command]
pub async fn intro_begin(app: AppHandle) -> bool {
    if !HELD.swap(false, Ordering::AcqRel) {
        tracing::info!("the intro plays inside the window: it wasn't held for it");
        return false;
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        let _ = tx.send(mac::stage(&handle));
    });
    let shown = queued.is_ok() && rx.await.unwrap_or(false);
    if !shown {
        tracing::warn!("the window couldn't be staged for the intro; it plays inside it");
        unhold(&app);
        return false;
    }
    tracing::info!("the window is staged for the intro");
    let armed = ENDS.load(Ordering::Acquire);
    std::thread::spawn(move || {
        std::thread::sleep(STAGED_DEADLINE);
        if ENDS.load(Ordering::Acquire) == armed && staged() {
            tracing::warn!("the intro ran past its deadline; ending it");
            end(&app, QUICK_FADE, true);
        }
    });
    true
}

/// The burst has opened the window: its title-bar buttons, shadow and colour come back.
#[tauri::command]
pub fn intro_shaped(app: AppHandle) {
    if !staged() {
        return;
    }
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || mac::shape(&handle));
}

/// Ends the intro: the window is restored in full and the dim fades out over `fade_ms`, then
/// closes. Does nothing once ended.
#[tauri::command]
pub fn intro_finish(app: AppHandle, fade_ms: u32) {
    end(&app, f64::from(fade_ms.min(5_000)) / 1000.0, false);
}

/// Shows a held window normally, without the intro (reduced motion, or no intro to play).
#[tauri::command]
pub fn intro_release(app: AppHandle) {
    if HELD.swap(false, Ordering::AcqRel) {
        tracing::info!("the window shows without the intro");
        unhold(&app);
    }
}

/// Ends a staged intro from the window's side (hidden, closed, its page loading again): the
/// dim goes at once, the window is restored, and the page is told.
pub fn interrupt(app: &AppHandle) {
    end(app, 0.0, true);
}

/// Ends a staged intro once; `tell_page` when the page didn't ask for it.
fn end(app: &AppHandle, fade: f64, tell_page: bool) {
    if !STAGED.swap(false, Ordering::AcqRel) {
        return;
    }
    ENDS.fetch_add(1, Ordering::AcqRel);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || mac::finish(&handle, fade));
    if tell_page {
        let _ = app.emit_to(MAIN_WINDOW, ENDED_EVENT, ());
    }
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod mac {
    use std::cell::RefCell;
    use std::ptr::NonNull;

    use block2::{RcBlock, StackBlock};
    use objc2::rc::Retained;
    use objc2::{MainThreadOnly, msg_send};
    use objc2_app_kit::{
        NSAnimatablePropertyContainer, NSAnimationContext, NSBackingStoreType, NSColor,
        NSFloatingWindowLevel, NSNormalWindowLevel, NSScreen, NSWindow, NSWindowButton,
        NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_foundation::{MainThreadMarker, NSRect};
    use objc2_quartz_core::CAMediaTimingFunction;
    use tauri::{AppHandle, Manager};

    use super::{MAIN_WINDOW, STAGED};
    use crate::shell;

    /// How dark the desktop gets behind the window.
    const DIM: f64 = 0.85;
    /// When the dim starts fading in, after the window is staged (just after the page's orb
    /// has come up: tokens.css), and how long it takes, in seconds.
    const DIM_AFTER: f64 = 1.1;
    const DIM_IN: f64 = 0.75;

    thread_local! {
        /// The dim over the desktop while the intro is staged (main thread only).
        static DIM_WINDOW: RefCell<Option<Retained<NSWindow>>> = const { RefCell::new(None) };
    }

    fn main_window(app: &AppHandle) -> Option<(tauri::WebviewWindow, Retained<NSWindow>)> {
        let window = app.get_webview_window(MAIN_WINDOW)?;
        let pointer = window.ns_window().ok()?.cast::<NSWindow>();
        // SAFETY: Tauri hands out the main window's live NSWindow, and we're on the main
        // thread, where AppKit objects may be used; retaining keeps it alive while we do.
        let ns_window = unsafe { Retained::retain(pointer) }?;
        Some((window, ns_window))
    }

    fn set_buttons_hidden(window: &NSWindow, hidden: bool) {
        for button in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ] {
            if let Some(button) = window.standardWindowButton(button) {
                button.setHidden(hidden);
            }
        }
    }

    /// Shows the main window clear and passing the mouse through. `false` if it can't be.
    pub fn hold(app: &AppHandle) -> bool {
        if MainThreadMarker::new().is_none() {
            return false;
        }
        let Some((_, ns_window)) = main_window(app) else {
            return false;
        };
        ns_window.setAlphaValue(0.0);
        ns_window.setIgnoresMouseEvents(true);
        shell::show_main(app);
        true
    }

    /// A held window shows as it is and takes the mouse again.
    pub fn unhold(app: &AppHandle) {
        let Some((_, ns_window)) = main_window(app) else {
            return;
        };
        ns_window.setIgnoresMouseEvents(false);
        ns_window.setAlphaValue(1.0);
        shell::show_main(app);
    }

    /// The window shows with nothing of its own (only the page draws), over the dim, which
    /// fades in a moment later. `false` if it couldn't be staged.
    pub fn stage(app: &AppHandle) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Some((window, ns_window)) = main_window(app) else {
            return false;
        };
        let Some(dim) = dim_window(mtm) else {
            return false;
        };
        STAGED.store(true, std::sync::atomic::Ordering::Release);
        while let Ok(true) = window_vibrancy::clear_vibrancy(&window) {}
        if let Err(err) = window.set_background_color(None) {
            tracing::warn!(error = %err, "could not clear the window's colour for the intro");
        }
        ns_window.setHasShadow(false);
        set_buttons_hidden(&ns_window, true);
        ns_window.setLevel(NSFloatingWindowLevel);
        ns_window.center();
        dim.orderFrontRegardless();
        // Brought forward after the dim, so it stands above it on the same level.
        ns_window.setIgnoresMouseEvents(false);
        ns_window.setAlphaValue(1.0);
        shell::show_main(app);
        DIM_WINDOW.with_borrow_mut(|slot| *slot = Some(dim));
        let handle = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs_f64(DIM_AFTER));
            // Only if the intro hasn't ended meanwhile (its end takes the dim away).
            let _ = handle.run_on_main_thread(|| {
                DIM_WINDOW.with_borrow(|slot| {
                    if let Some(dim) = slot {
                        fade(dim, DIM, DIM_IN, None);
                    }
                });
            });
        });
        true
    }

    /// The window's own title-bar buttons, shadow and colour come back.
    pub fn shape(app: &AppHandle) {
        let Some((window, ns_window)) = main_window(app) else {
            return;
        };
        ns_window.setHasShadow(true);
        set_buttons_hidden(&ns_window, false);
        if let Err(err) = window.set_background_color(Some(crate::WINDOW_BACKGROUND)) {
            tracing::warn!(error = %err, "could not restore the window's colour after the intro");
        }
    }

    /// Restores the window in full and fades the dim out over `fade` seconds, then closes it.
    /// The window stays above the dim until it has gone, so the dim never darkens it.
    pub fn finish(app: &AppHandle, fade_secs: f64) {
        shape(app);
        let window = main_window(app).map(|(_, ns_window)| ns_window);
        let lower = move || {
            if let Some(window) = &window {
                window.setLevel(NSNormalWindowLevel);
            }
        };
        let Some(dim) = DIM_WINDOW.with_borrow_mut(Option::take) else {
            lower();
            return;
        };
        if fade_secs <= 0.0 {
            dim.close();
            lower();
            return;
        }
        let closing = dim.clone();
        let done = RcBlock::new(move || {
            closing.close();
            lower();
        });
        fade(&dim, 0.0, fade_secs, Some(done));
    }

    /// A borderless black window over the main screen below the menu bar, clear at first,
    /// that the mouse passes through and that never takes the focus.
    fn dim_window(mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
        let screen = NSScreen::mainScreen(mtm)?;
        let frame = screen.frame();
        let visible = screen.visibleFrame();
        let menu_bar =
            (frame.origin.y + frame.size.height) - (visible.origin.y + visible.size.height);
        let mut rect: NSRect = frame;
        rect.size.height -= menu_bar.max(0.0);
        // SAFETY: a plain window made on the main thread with a valid frame, style and
        // backing; it is kept (not released) when closed, as we hold it.
        let window = unsafe {
            let window = NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                rect,
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            );
            window.setReleasedWhenClosed(false);
            window
        };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::blackColor()));
        window.setAlphaValue(0.0);
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setLevel(NSFloatingWindowLevel);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        // SAFETY: `setExcludedFromWindowsMenu:` takes a BOOL and the window is live.
        let () = unsafe { msg_send![&window, setExcludedFromWindowsMenu: true] };
        Some(window)
    }

    /// Animates `window`'s alpha to `to` over `secs`, easing in and out; `done` runs after.
    fn fade(window: &NSWindow, to: f64, secs: f64, done: Option<RcBlock<dyn Fn()>>) {
        let easing = CAMediaTimingFunction::functionWithControlPoints(0.42, 0.0, 0.58, 1.0);
        let changes = StackBlock::new(|context: NonNull<NSAnimationContext>| {
            // SAFETY: AppKit passes the group's live context for the length of this call.
            let context = unsafe { context.as_ref() };
            context.setDuration(secs);
            context.setTimingFunction(Some(&easing));
            window.animator().setAlphaValue(to);
        });
        NSAnimationContext::runAnimationGroup_completionHandler(&changes, done.as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
mod mac {
    use tauri::AppHandle;

    pub fn hold(_app: &AppHandle) -> bool {
        false
    }

    pub fn unhold(_app: &AppHandle) {}

    pub fn stage(_app: &AppHandle) -> bool {
        false
    }

    pub fn shape(_app: &AppHandle) {}

    pub fn finish(_app: &AppHandle, _fade_secs: f64) {}
}
