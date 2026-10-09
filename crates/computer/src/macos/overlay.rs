//! The agent cursor's overlay (§4.5): one transparent panel per display, above every window,
//! on every Space, that the mouse goes through. Each worker's cursor is an arrow in its colour
//! with its name in a pill; it glides to each aim, pulses on a click and outlines the element
//! an element action names. It never moves the real cursor and never posts an event.
//!
//! Callers on any thread hand a message to the main queue and return at once. On the main
//! thread the message goes through the [`CursorScene`], and what changed is drawn with Core
//! Animation layers, so the render server runs the motion and no thread waits on it.
#![allow(unsafe_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont, NSPanel,
    NSResponder, NSScreen, NSScreenSaverWindowLevel, NSView, NSWindow, NSWindowAnimationBehavior,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFRetained, CFType, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGColor, CGMutablePath, CGPath};
use objc2_foundation::{NSArray, NSNumber, NSObject, NSString, NSValue};
use objc2_quartz_core::{
    CABasicAnimation, CACurrentMediaTime, CAKeyframeAnimation, CALayer, CAMediaTiming,
    CAMediaTimingFunction, CAShapeLayer, CATextLayer, CATransaction, kCAAlignmentCenter,
    kCAMediaTimingFunctionEaseInEaseOut, kCAMediaTimingFunctionEaseOut, kCATruncationEnd,
};

use crate::cursor::{
    Aim, Change, CursorMsg, CursorScene, CursorSink, FADE, Gesture, IDLE, Phase, WorkerCursor,
};
use crate::geom::{Point, Rect};

/// The glide to a new point.
const GLIDE: f64 = 0.15;
/// A drag's glide: to where it takes hold, then to where it lets go.
const DRAG: f64 = 0.5;
/// How long an element's outline shows.
const OUTLINE: f64 = 1.0;
/// The pulse on a click, starting as the glide lands.
const PULSE: f64 = 0.45;
/// How much later than due a fade timer fires, so the scene sees the time as passed.
const TIMER_SLACK: Duration = Duration::from_millis(20);

/// The arrow, tip at the origin, y down: a classic pointer about 20 pt tall.
const ARROW: [(f64, f64); 7] = [
    (0.0, 0.0),
    (0.0, 17.0),
    (4.3, 13.4),
    (7.1, 19.6),
    (9.9, 18.4),
    (7.1, 12.3),
    (12.6, 12.3),
];
/// Where the pill's top left sits, from the tip.
const PILL_AT: (f64, f64) = (11.0, 20.0);
const PILL_FONT: f64 = 11.0;
const PILL_PAD: (f64, f64) = (7.0, 2.5);

/// The overlay as the engine and the helper see it. Every call returns at once.
pub struct Overlay(());

impl Overlay {
    /// The overlay draws once the main thread runs AppKit; it builds its panels then, on the
    /// first message.
    pub fn new() -> Arc<Self> {
        Arc::new(Self(()))
    }

    fn send(&self, msg: CursorMsg) {
        DispatchQueue::main().exec_async(move || on_main(msg));
    }
}

impl CursorSink for Overlay {
    fn aim(&self, aim: Aim) {
        self.send(CursorMsg::Aim(aim));
    }
    fn label(&self, worker: &str, label: &str) {
        self.send(CursorMsg::Label {
            worker: worker.to_owned(),
            label: label.to_owned(),
        });
    }
    fn end(&self, worker: &str) {
        self.send(CursorMsg::End(worker.to_owned()));
    }
    fn clear(&self) {
        self.send(CursorMsg::Clear);
    }
}

thread_local! {
    /// The drawing, on the main thread only.
    static DRAWING: RefCell<Option<Drawing>> = const { RefCell::new(None) };
}

fn on_main(msg: CursorMsg) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    DRAWING.with(|d| {
        let mut d = d.borrow_mut();
        d.get_or_insert_with(Drawing::default).apply(mtm, msg);
    });
}

/// Asks for a tick once `after` has passed. A tick that finds nothing due does nothing.
fn tick_in(after: Duration) {
    let Ok(when) = DispatchTime::try_from(after + TIMER_SLACK) else {
        return;
    };
    let _ = DispatchQueue::main().after(when, || on_main(CursorMsg::Tick));
}

#[derive(Default)]
struct Drawing {
    scene: CursorScene,
    /// The screens the panels were built for: their frames and backing scales.
    screens: Vec<(CGRect, CGFloat)>,
    panels: Vec<Panel>,
}

impl Drawing {
    fn apply(&mut self, mtm: MainThreadMarker, msg: CursorMsg) {
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        self.sync_screens(mtm);
        for change in self.scene.apply(Instant::now(), msg) {
            match change {
                Change::Moved { aim, appeared } => {
                    if let Some(c) = self.scene.get(&aim.worker) {
                        for p in &mut self.panels {
                            p.moved(&aim, c, appeared);
                        }
                    }
                    tick_in(IDLE);
                }
                Change::Labelled { worker } => {
                    if let Some(c) = self.scene.get(&worker) {
                        for p in &mut self.panels {
                            if let Some(s) = p.sprites.get(&worker) {
                                s.set_label(c.label.as_deref());
                            }
                        }
                    }
                }
                Change::Fade { worker } => {
                    for p in &mut self.panels {
                        if let Some(s) = p.sprites.get(&worker) {
                            s.fade();
                        }
                    }
                    tick_in(FADE);
                }
                Change::Removed { worker } => {
                    for p in &mut self.panels {
                        if let Some(s) = p.sprites.remove(&worker) {
                            s.remove();
                        }
                    }
                }
            }
        }
        CATransaction::commit();
    }

    /// Builds a panel for each screen, again whenever the screens change, with the cursors
    /// that are shown drawn where they are.
    fn sync_screens(&mut self, mtm: MainThreadMarker) {
        let screens: Vec<(CGRect, CGFloat)> = NSScreen::screens(mtm)
            .iter()
            .map(|s| (s.frame(), s.backingScaleFactor()))
            .collect();
        if screens == self.screens {
            return;
        }
        for p in self.panels.drain(..) {
            p.close();
        }
        // Cocoa's screen frames start at the bottom left of the first screen (the one with
        // the menu bar); the scene's points start at its top left.
        let main_height = screens.first().map_or(0.0, |(f, _)| f.size.height);
        self.panels = screens
            .iter()
            .map(|&(frame, scale)| Panel::new(mtm, frame, scale, main_height))
            .collect();
        self.screens = screens;
        for (worker, c) in self.scene.cursors() {
            if c.phase != Phase::Shown {
                continue;
            }
            let aim = Aim {
                worker: worker.to_owned(),
                at: c.at,
                element: None,
                gesture: Gesture::Scroll,
            };
            for p in &mut self.panels {
                p.moved(&aim, c, true);
            }
        }
    }
}

define_class!(
    // SAFETY: NSPanel has no subclassing requirements, and `OverlayPanel` adds no ivars and
    // doesn't implement `Drop`.
    #[unsafe(super(NSPanel, NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "BrigadierComputerOverlayPanel"]
    struct OverlayPanel;

    impl OverlayPanel {
        /// It never takes the keyboard.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            false
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            false
        }
    }

    unsafe impl NSObjectProtocol for OverlayPanel {}
);

impl OverlayPanel {
    fn new(mtm: MainThreadMarker, frame: CGRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: NSPanel's designated initializer, with a frame, a style mask and a backing
        // store type it takes.
        unsafe {
            msg_send![
                super(this),
                initWithContentRect: frame,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        }
    }
}

/// One display's panel and the layers it draws each worker with.
struct Panel {
    window: Retained<OverlayPanel>,
    root: Retained<CALayer>,
    /// The panel's top left, in the scene's global points.
    origin: Point,
    scale: CGFloat,
    sprites: HashMap<String, Sprite>,
}

impl Panel {
    fn new(mtm: MainThreadMarker, frame: CGRect, scale: CGFloat, main_height: f64) -> Self {
        let window = OverlayPanel::new(mtm, frame);
        // SAFETY: the panel is held by `Panel` and closed by it; AppKit must not release it
        // on close as well.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setOpaque(false);
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setHasShadow(false);
        window.setIgnoresMouseEvents(true);
        window.setHidesOnDeactivate(false);
        window.setLevel(NSScreenSaverWindowLevel);
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        let bounds = CGRect::new(CGPoint::ZERO, frame.size);
        let view = NSView::initWithFrame(NSView::alloc(mtm), bounds);
        // The view hosts the layer. AppKit sets the view's own layer's flip, so the cursors
        // go on a flipped layer inside it, where points go down from the top left as the
        // scene's do.
        let host = CALayer::new();
        host.setFrame(bounds);
        view.setLayer(Some(&host));
        view.setWantsLayer(true);
        let root = CALayer::new();
        root.setGeometryFlipped(true);
        root.setFrame(bounds);
        host.addSublayer(&root);
        window.setContentView(Some(&view));
        // Shown without activating the helper, which never activates.
        window.orderFrontRegardless();
        let origin = Point::new(
            frame.origin.x,
            main_height - (frame.origin.y + frame.size.height),
        );
        Self {
            window,
            root,
            origin,
            scale,
            sprites: HashMap::new(),
        }
    }

    /// A global point in the panel's layer points.
    fn local(&self, p: Point) -> CGPoint {
        CGPoint::new(p.x - self.origin.x, p.y - self.origin.y)
    }

    fn moved(&mut self, aim: &Aim, c: &WorkerCursor, appeared: bool) {
        if !self.sprites.contains_key(&aim.worker) {
            let s = Sprite::new(&self.root, c, self.scale);
            self.sprites.insert(aim.worker.clone(), s);
        }
        let at = self.local(aim.at);
        let to = match aim.gesture {
            Gesture::Drag { to } => Some(self.local(to)),
            _ => None,
        };
        let element = aim.element.map(|e| self.local_rect(e));
        let s = &self.sprites[&aim.worker];
        s.show(appeared);
        s.glide(at, to, appeared);
        if let (Gesture::Press | Gesture::Type, Some(e)) = (aim.gesture, element) {
            s.outline(e);
        }
        if matches!(aim.gesture, Gesture::Click | Gesture::Press) {
            s.pulse(at, if appeared { 0.0 } else { GLIDE * 0.8 });
        }
    }

    fn local_rect(&self, r: Rect) -> CGRect {
        CGRect::new(self.local(Point::new(r.x, r.y)), CGSize::new(r.w, r.h))
    }

    fn close(self) {
        for (_, s) in self.sprites {
            s.remove();
        }
        self.window.orderOut(None);
        self.window.close();
    }
}

/// One worker's layers on one panel: the arrow and its pill move together in `group`; the
/// outline and the pulse stay where they were drawn.
struct Sprite {
    group: Retained<CALayer>,
    pill: Retained<CALayer>,
    text: Retained<CATextLayer>,
    outline: Retained<CAShapeLayer>,
    ring: Retained<CAShapeLayer>,
}

fn color((r, g, b): (u8, u8, u8), alpha: f64) -> CFRetained<CGColor> {
    CGColor::new_srgb(
        f64::from(r) / 255.0,
        f64::from(g) / 255.0,
        f64::from(b) / 255.0,
        alpha,
    )
}

fn white(alpha: f64) -> CFRetained<CGColor> {
    CGColor::new_srgb(1.0, 1.0, 1.0, alpha)
}

fn black(alpha: f64) -> CFRetained<CGColor> {
    CGColor::new_srgb(0.0, 0.0, 0.0, alpha)
}

fn arrow_path() -> CFRetained<CGMutablePath> {
    let path = CGMutablePath::new();
    for (i, &(x, y)) in ARROW.iter().enumerate() {
        // SAFETY: a null transform means none.
        unsafe {
            if i == 0 {
                CGMutablePath::move_to_point(Some(&path), std::ptr::null(), x, y);
            } else {
                CGMutablePath::add_line_to_point(Some(&path), std::ptr::null(), x, y);
            }
        }
    }
    CGMutablePath::close_subpath(Some(&path));
    path
}

fn rounded(rect: CGRect, radius: f64) -> CFRetained<CGPath> {
    // SAFETY: a null transform means none; the radius is under half of either side.
    let r = radius
        .min(rect.size.width / 2.0)
        .min(rect.size.height / 2.0);
    unsafe { CGPath::with_rounded_rect(rect, r, r, std::ptr::null()) }
}

fn number(v: f64) -> Retained<NSNumber> {
    NSNumber::new_f64(v)
}

fn point_value(p: CGPoint) -> Retained<NSValue> {
    NSValue::new(p)
}

fn animation(
    key_path: &str,
    from: &AnyObject,
    to: &AnyObject,
    duration: f64,
) -> Retained<CABasicAnimation> {
    let a = CABasicAnimation::animationWithKeyPath(Some(&NSString::from_str(key_path)));
    // SAFETY: the values are of the key path's type: the callers pair them.
    unsafe {
        a.setFromValue(Some(from));
        a.setToValue(Some(to));
    }
    a.setDuration(duration);
    a
}

impl Sprite {
    fn new(root: &CALayer, c: &WorkerCursor, scale: CGFloat) -> Self {
        let fill = color(c.color, 1.0);
        let group = CALayer::new();
        group.setBounds(CGRect::ZERO);
        group.setPosition(CGPoint::ZERO);

        let arrow = CAShapeLayer::new();
        arrow.setContentsScale(scale);
        let path = arrow_path();
        let path: &CGPath = &path;
        arrow.setPath(Some(path));
        arrow.setFillColor(Some(&fill));
        arrow.setStrokeColor(Some(&white(1.0)));
        arrow.setLineWidth(1.5);
        // SAFETY: a line-join name the layer takes.
        arrow.setLineJoin(unsafe { objc2_quartz_core::kCALineJoinRound });
        arrow.setShadowPath(Some(path));
        arrow.setShadowColor(Some(&black(1.0)));
        arrow.setShadowOpacity(0.35);
        arrow.setShadowRadius(2.0);
        arrow.setShadowOffset(CGSize::new(0.0, 1.0));
        group.addSublayer(&arrow);

        let pill = CALayer::new();
        pill.setContentsScale(scale);
        pill.setBackgroundColor(Some(&fill));
        pill.setBorderColor(Some(&white(0.9)));
        pill.setBorderWidth(1.0);
        pill.setShadowColor(Some(&black(1.0)));
        pill.setShadowOpacity(0.25);
        pill.setShadowRadius(2.0);
        pill.setShadowOffset(CGSize::new(0.0, 1.0));
        let text = CATextLayer::new();
        text.setContentsScale(scale);
        let font = NSFont::boldSystemFontOfSize(PILL_FONT);
        // SAFETY: NSFont is toll-free bridged with CTFont, a CF type the layer takes as its
        // font.
        unsafe {
            let font: &CFType = &*(Retained::as_ptr(&font).cast::<CFType>());
            text.setFont(Some(font));
        }
        text.setFontSize(PILL_FONT);
        text.setForegroundColor(Some(&white(1.0)));
        // SAFETY: names the layer takes.
        unsafe {
            text.setAlignmentMode(kCAAlignmentCenter);
            text.setTruncationMode(kCATruncationEnd);
        }
        // A faint shadow keeps white text readable on the lighter colours.
        text.setShadowColor(Some(&black(1.0)));
        text.setShadowOpacity(0.3);
        text.setShadowRadius(1.0);
        text.setShadowOffset(CGSize::new(0.0, 0.5));
        pill.addSublayer(&text);
        group.addSublayer(&pill);

        let outline = CAShapeLayer::new();
        outline.setContentsScale(scale);
        outline.setFillColor(None);
        outline.setStrokeColor(Some(&fill));
        outline.setLineWidth(2.0);
        outline.setOpacity(0.0);

        let ring = CAShapeLayer::new();
        ring.setContentsScale(scale);
        let d = 28.0;
        // SAFETY: a null transform means none.
        let circle = unsafe {
            CGPath::with_ellipse_in_rect(
                CGRect::new(CGPoint::new(-d / 2.0, -d / 2.0), CGSize::new(d, d)),
                std::ptr::null(),
            )
        };
        ring.setPath(Some(&circle));
        ring.setFillColor(Some(&color(c.color, 0.25)));
        ring.setStrokeColor(Some(&fill));
        ring.setLineWidth(2.0);
        ring.setOpacity(0.0);

        // Outlines and pulses under the cursors.
        root.insertSublayer_atIndex(&outline, 0);
        root.insertSublayer_atIndex(&ring, 0);
        root.addSublayer(&group);
        let s = Self {
            group,
            pill,
            text,
            outline,
            ring,
        };
        s.set_label(c.label.as_deref());
        s
    }

    fn set_label(&self, label: Option<&str>) {
        let Some(label) = label.filter(|l| !l.is_empty()) else {
            self.pill.setHidden(true);
            return;
        };
        let s = NSString::from_str(label);
        let obj: &AnyObject = &s;
        // SAFETY: a string is what the layer draws.
        unsafe { self.text.setString(Some(obj)) };
        let size = self.text.preferredFrameSize();
        let (w, h) = (size.width.ceil(), size.height.ceil());
        self.text.setFrame(CGRect::new(
            CGPoint::new(PILL_PAD.0, PILL_PAD.1),
            CGSize::new(w, h),
        ));
        let pill = CGSize::new(w + PILL_PAD.0 * 2.0, h + PILL_PAD.1 * 2.0);
        self.pill
            .setFrame(CGRect::new(CGPoint::new(PILL_AT.0, PILL_AT.1), pill));
        self.pill.setCornerRadius(pill.height / 2.0);
        self.pill.setHidden(false);
    }

    /// Shows the cursor fully again, ending a fade; one that just appeared fades in.
    fn show(&self, appeared: bool) {
        self.group
            .removeAnimationForKey(&NSString::from_str("fade"));
        self.group.setOpacity(1.0);
        if appeared {
            let a = animation("opacity", &number(0.0), &number(1.0), 0.12);
            self.group
                .addAnimation_forKey(&a, Some(&NSString::from_str("fade")));
        }
    }

    /// Glides the tip to `at` (then to `to`, for a drag) from wherever it is on screen now.
    fn glide(&self, at: CGPoint, to: Option<CGPoint>, appeared: bool) {
        // SAFETY: a plain query; the copy it returns is only read.
        let now = unsafe { self.group.presentationLayer() }
            .map_or_else(|| self.group.position(), |p| p.position());
        let from = if appeared { at } else { now };
        let end = to.unwrap_or(at);
        self.group.setPosition(end);
        let key = NSString::from_str("glide");
        match to {
            None if !appeared => {
                let a = animation("position", &point_value(from), &point_value(at), GLIDE);
                a.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(
                    // SAFETY: a timing function name.
                    unsafe { kCAMediaTimingFunctionEaseOut },
                )));
                self.group.addAnimation_forKey(&a, Some(&key));
            }
            None => {}
            Some(to) => {
                let a = CAKeyframeAnimation::animationWithKeyPath(Some(&NSString::from_str(
                    "position",
                )));
                let values = [point_value(from), point_value(at), point_value(to)];
                let values: Vec<&AnyObject> = values.iter().map(|v| -> &AnyObject { v }).collect();
                let lead = if appeared { 0.0 } else { GLIDE / DRAG };
                let times = NSArray::from_retained_slice(&[number(0.0), number(lead), number(1.0)]);
                // SAFETY: points for a position key path, a key time for each.
                unsafe { a.setValues(Some(&NSArray::from_slice(&values))) };
                a.setKeyTimes(Some(&times));
                // SAFETY: timing function names.
                let (out, in_out) = unsafe {
                    (
                        kCAMediaTimingFunctionEaseOut,
                        kCAMediaTimingFunctionEaseInEaseOut,
                    )
                };
                a.setTimingFunctions(Some(&NSArray::from_retained_slice(&[
                    CAMediaTimingFunction::functionWithName(out),
                    CAMediaTimingFunction::functionWithName(in_out),
                ])));
                a.setDuration(DRAG);
                self.group.addAnimation_forKey(&a, Some(&key));
            }
        }
    }

    /// Outlines an element's frame for a moment.
    fn outline(&self, frame: CGRect) {
        let r = CGRect::new(
            CGPoint::new(frame.origin.x - 2.0, frame.origin.y - 2.0),
            CGSize::new(frame.size.width + 4.0, frame.size.height + 4.0),
        );
        self.outline.setPath(Some(&rounded(r, 5.0)));
        let a = CAKeyframeAnimation::animationWithKeyPath(Some(&NSString::from_str("opacity")));
        let values = [number(1.0), number(1.0), number(0.0)];
        let values: Vec<&AnyObject> = values.iter().map(|v| -> &AnyObject { v }).collect();
        // SAFETY: numbers for an opacity key path, a key time for each.
        unsafe { a.setValues(Some(&NSArray::from_slice(&values))) };
        a.setKeyTimes(Some(&NSArray::from_retained_slice(&[
            number(0.0),
            number(0.7),
            number(1.0),
        ])));
        a.setDuration(OUTLINE);
        self.outline
            .addAnimation_forKey(&a, Some(&NSString::from_str("show")));
    }

    /// A ring that grows and fades where a click lands, `delay` seconds from now.
    fn pulse(&self, at: CGPoint, delay: f64) {
        self.ring.setPosition(at);
        let begin = self.ring.convertTime_fromLayer(CACurrentMediaTime(), None) + delay;
        let grow = animation("transform.scale", &number(0.3), &number(1.4), PULSE);
        let fade = animation("opacity", &number(0.9), &number(0.0), PULSE);
        for (a, key) in [(grow, "grow"), (fade, "fade")] {
            a.setBeginTime(begin);
            a.setTimingFunction(Some(&CAMediaTimingFunction::functionWithName(
                // SAFETY: a timing function name.
                unsafe { kCAMediaTimingFunctionEaseOut },
            )));
            self.ring
                .addAnimation_forKey(&a, Some(&NSString::from_str(key)));
        }
    }

    fn fade(&self) {
        // SAFETY: a plain query; the copy it returns is only read.
        let now = unsafe { self.group.presentationLayer() }.map_or(1.0, |p| p.opacity());
        self.group.setOpacity(0.0);
        let a = animation(
            "opacity",
            &number(f64::from(now)),
            &number(0.0),
            FADE.as_secs_f64(),
        );
        self.group
            .addAnimation_forKey(&a, Some(&NSString::from_str("fade")));
    }

    fn remove(&self) {
        self.group.removeFromSuperlayer();
        self.outline.removeFromSuperlayer();
        self.ring.removeFromSuperlayer();
    }
}

/// Runs AppKit on this thread, the process's main thread, as an accessory app that never
/// activates, so the overlay draws, while `work` runs on its own thread with it. The process
/// exits with `work`'s exit code.
pub fn run_with_overlay<F>(mtm: MainThreadMarker, work: F) -> !
where
    F: FnOnce(Arc<Overlay>) -> i32 + Send + 'static,
{
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let overlay = Overlay::new();
    let spawned = std::thread::Builder::new()
        .name("computer-overlay-work".into())
        .spawn(move || {
            // A panic here would leave the main thread drawing forever; it ends the process.
            let code = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(overlay)))
                .unwrap_or(101);
            std::process::exit(code)
        });
    if let Err(e) = spawned {
        eprintln!("brigadier-computer: {e}");
        std::process::exit(1);
    }
    app.run();
    std::process::exit(0)
}

/// Two workers' cursors gliding over the main display for a few seconds; one session ends,
/// then the user's stop clears the other. Draws only: it sends no input anywhere.
pub fn demo(cursor: &dyn CursorSink) -> i32 {
    let main = objc2_core_graphics::CGDisplayBounds(objc2_core_graphics::CGMainDisplayID());
    let (w, h) = (main.size.width, main.size.height);
    cursor.label("demo-a", "Fix the login page");
    cursor.label("demo-b", "Write the release notes for October");
    let step = Duration::from_millis(250);
    for i in 0..12 {
        let t = f64::from(i) / 12.0 * std::f64::consts::TAU;
        let a = Point::new(w * 0.35 + 120.0 * t.cos(), h * 0.4 + 80.0 * t.sin());
        let b = Point::new(w * 0.62 - 140.0 * t.sin(), h * 0.55 + 60.0 * t.cos());
        let gesture = match i % 4 {
            0 => Gesture::Click,
            1 => Gesture::Press,
            2 => Gesture::Type,
            _ => Gesture::Scroll,
        };
        let element = matches!(gesture, Gesture::Press | Gesture::Type)
            .then(|| Rect::new(b.x - 60.0, b.y - 12.0, 120.0, 24.0));
        cursor.aim(Aim {
            worker: "demo-a".into(),
            at: a,
            element: None,
            gesture: Gesture::Click,
        });
        cursor.aim(Aim {
            worker: "demo-b".into(),
            at: b,
            element,
            gesture,
        });
        std::thread::sleep(step);
    }
    cursor.aim(Aim {
        worker: "demo-a".into(),
        at: Point::new(w * 0.3, h * 0.3),
        element: None,
        gesture: Gesture::Drag {
            to: Point::new(w * 0.45, h * 0.35),
        },
    });
    std::thread::sleep(Duration::from_secs(1));
    cursor.end("demo-a");
    std::thread::sleep(Duration::from_secs(1));
    cursor.clear();
    std::thread::sleep(Duration::from_millis(500));
    0
}
