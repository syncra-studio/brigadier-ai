//! The helper's main thread: an accessory AppKit app with a menu-bar item and the ⌃⌥⌘.
//! hotkey, both of which stop every worker's computer use (§4.7). Neither waits on engine
//! work: a stop bumps the global generation and hands the events to another thread.
#![allow(unsafe_code)]

use std::ptr::NonNull;
use std::sync::Arc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventMask, NSEventModifierFlags,
    NSImage, NSMenu, NSMenuItem, NSStatusBar, NSVariableStatusItemLength,
};
use objc2_foundation::NSString;

use super::hub::Hub;

/// The period key's virtual keycode.
const PERIOD: u16 = 47;

struct Ivars {
    hub: Arc<Hub>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `StopTarget` doesn't implement
    // `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BrigadierComputerStopTarget"]
    #[ivars = Ivars]
    struct StopTarget;

    impl StopTarget {
        #[unsafe(method(stop:))]
        fn stop(&self, _sender: Option<&AnyObject>) {
            self.ivars().hub.stop("menu");
        }
    }

    unsafe impl NSObjectProtocol for StopTarget {}
);

impl StopTarget {
    fn new(mtm: MainThreadMarker, hub: Arc<Hub>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { hub });
        // SAFETY: NSObject's `init` on a freshly allocated object.
        unsafe { msg_send![super(this), init] }
    }
}

/// The modifiers that count for the hotkey; caps lock and the like are ignored.
fn hotkey_mods() -> NSEventModifierFlags {
    NSEventModifierFlags::Control | NSEventModifierFlags::Option | NSEventModifierFlags::Command
}

/// Runs AppKit on this thread, the process's main thread, until the process exits.
pub fn run(mtm: MainThreadMarker, hub: Arc<Hub>) -> ! {
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
    if let Some(button) = item.button(mtm) {
        let name = NSString::from_str("Brigadier Computer Use");
        match NSImage::imageWithSystemSymbolName_accessibilityDescription(
            &NSString::from_str("cursorarrow.rays"),
            Some(&name),
        ) {
            Some(image) => {
                image.setTemplate(true);
                button.setImage(Some(&image));
            }
            None => button.setTitle(&NSString::from_str("CU")),
        }
    }

    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    // SAFETY: no action selector; the empty key equivalent means none.
    let info = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Brigadier Computer Use"),
            None,
            &NSString::new(),
        )
    };
    info.setEnabled(false);
    menu.addItem(&info);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let target = StopTarget::new(mtm, hub.clone());
    // SAFETY: `stop:` is the target's method, taking the sender.
    let stop = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Stop computer use"),
            Some(sel!(stop:)),
            &NSString::from_str("."),
        )
    };
    stop.setKeyEquivalentModifierMask(hotkey_mods());
    // SAFETY: the target responds to the item's action and outlives it: both live until
    // `run` below returns, which it never does.
    unsafe { stop.setTarget(Some(&target)) };
    menu.addItem(&stop);
    item.setMenu(Some(&menu));

    // The hotkey works while another app is in front. The system delivers key events to a
    // global monitor only once Accessibility is granted, which computer use needs anyway.
    let h = hub.clone();
    let on_key = RcBlock::new(move |event: NonNull<NSEvent>| {
        // SAFETY: the system passes a live event for the duration of the call.
        let event = unsafe { event.as_ref() };
        let relevant = NSEventModifierFlags::Shift | hotkey_mods();
        if event.keyCode() == PERIOD && event.modifierFlags() & relevant == hotkey_mods() {
            h.stop("hotkey");
        }
    });
    let _monitor =
        NSEvent::addGlobalMonitorForEventsMatchingMask_handler(NSEventMask::KeyDown, &on_key);

    app.run();
    // `run` returns only if something terminated the app; the helper is done then.
    drop((target, item, menu, on_key));
    std::process::exit(0)
}
