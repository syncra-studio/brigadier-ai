//! The engine's side of pages in a browser the session launched (§8, Phase 5): the same
//! `observe`, `act` and `zoom`, the same refs, diffs, records and cursor, with the page reached
//! through the browser's debugging protocol instead of system events. Nothing here posts an
//! event to the system, activates the browser or needs its window on screen.

use super::*;
use crate::cdp::page::{self as web_page, DIALOG_ACCEPT, DIALOG_BOX, DIALOG_DISMISS, DIALOG_TEXT};
use crate::cdp::{Browser, Conn, Page, Viewport, WebEl, image, input};

/// A launched browser's pid and one of its tabs.
pub(super) type PageId = (i32, String);

/// What a page action did, beyond its rung.
#[derive(Default)]
struct Outcome {
    aim: Option<(Point, Option<Rect>)>,
    before: Option<RawNode<WebEl>>,
    /// Text that must read back from an element.
    text: Option<(WebEl, String)>,
    /// A `<select>` that must show this option.
    option: Option<(WebEl, String)>,
    answered: bool,
}

/// A target's point in main-viewport CSS pixels, its window point and box, and the element.
type WebPoint = ((f64, f64), (Point, Option<Rect>), Option<WebEl>);

impl<D: Desktop> Engine<D> {
    /// The page window `w` shows, when it is a window of a browser the session launched.
    pub(super) fn web_page(&mut self, w: &WindowInfo) -> Option<PageId> {
        if !self.web.pids().contains(&w.pid) && !self.adopt_browser(w.pid) {
            return None;
        }
        self.web.page_of(w)
    }

    /// Takes on a browser another helper launched for a session, found by its scratch
    /// profile (`cdp::adoptable`); any other browser is remembered as not ours.
    fn adopt_browser(&mut self, pid: i32) -> bool {
        if self.web.foreign.contains(&pid) {
            return false;
        }
        let adopted = (|| {
            let app = self.desktop.app(pid).ok()?;
            if !crate::cdp::is_chromium(app.bundle_id.as_deref()) {
                return None;
            }
            let profile = crate::cdp::adoptable(pid)?;
            let path = app.bundle_path.clone().unwrap_or_default();
            Browser::attach(pid, &path, profile, Duration::ZERO).ok()
        })();
        match adopted {
            Some(b) => {
                self.web.add(b);
                true
            }
            None => {
                self.web.foreign.insert(pid);
                false
            }
        }
    }

    fn browser(&mut self, pid: i32) -> CuResult<&mut Browser> {
        self.web
            .browser(pid)
            .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, "that browser quit"))
    }

    fn on_page<T>(
        &mut self,
        (pid, target): &PageId,
        f: impl FnOnce(&mut Page, &mut Conn) -> CuResult<T>,
    ) -> CuResult<T> {
        self.browser(*pid)?.with_page(target, f)
    }

    pub(super) fn web_observe(
        &mut self,
        worker: &str,
        req: &ObserveRequest,
        w: &WindowInfo,
        page: PageId,
    ) -> CuResult<Reply> {
        let element = match req.element.as_deref() {
            Some(e) => Some(
                tree::parse_ref(e)
                    .ok_or_else(|| CuError::new(ErrorCode::BadRequest, format!("bad ref {e}")))?,
            ),
            None => None,
        };
        if let (Some(r), Some(n)) = (element, req.value_page) {
            return self.web_value_page(w, &page, r, n);
        }
        let snap = self.browser(page.0)?.snapshot(&page.1, w)?;
        let refs = self.web_refs.entry(w.id).or_default();
        // A new document: every ref from the old one is stale.
        if self
            .web_docs
            .insert(w.id, snap.loader.clone())
            .is_some_and(|old| old != snap.loader)
        {
            refs.navigate();
        }
        let lines = refs.assign(&snap.nodes);
        let secure = Self::secure_frames(&snap.nodes);
        self.last_secure = (w.id, secure.clone());
        let want = match req.screenshot {
            Screenshot::Always => true,
            Screenshot::Auto => Self::tree_is_poor(&snap.nodes, &lines),
            Screenshot::Never => false,
        };
        // A page waiting on its dialog draws nothing new; its picture would only show the page.
        let image = if want && snap.dialog.is_none() {
            Some(self.web_shot(&page, w, &snap.viewport, &secure)?)
        } else {
            None
        };
        let mut head = format!("page {}\n", tree::quote(&snap.url, tree::VALUE_CLIP));
        if let Some(d) = &snap.dialog {
            let _ = writeln!(
                head,
                "the page waits on its {}: answer it with its accept or dismiss button",
                d.kind
            );
        }
        self.render_observation(worker, req, w, element, &lines, &head, None, image, false)
    }

    fn web_value_page(
        &mut self,
        w: &WindowInfo,
        page: &PageId,
        r: u32,
        n: usize,
    ) -> CuResult<Reply> {
        let el = self.web_resolve(w, page, r, false)?;
        let value: Vec<char> = self
            .on_page(page, |_, c| input::text_of(c, &el))?
            .chars()
            .collect();
        let pages = value.len().div_ceil(VALUE_PAGE).max(1);
        let start = (n * VALUE_PAGE).min(value.len());
        let end = (start + VALUE_PAGE).min(value.len());
        let chunk: String = value[start..end].iter().collect();
        Ok(Reply {
            text: format!(
                "e{r} value page {} of {pages} ({} chars in all):\n{chunk}\n",
                n + 1,
                value.len()
            ),
            ..Default::default()
        })
    }

    /// The viewport as an image at one pixel per point, password fields painted over.
    fn web_shot(
        &mut self,
        page: &PageId,
        w: &WindowInfo,
        vp: &Viewport,
        secure: &[Rect],
    ) -> CuResult<ImageOut> {
        let src = self.on_page(page, |p, c| p.screenshot(c))?;
        let crop = vp.rect();
        let t = ImageTransform::fit(w.id, w.frame, crop, 1.0, geom::MAX_IMAGE_SIDE);
        let img = image::resample(
            &src,
            Rect::new(0.0, 0.0, f64::from(src.width), f64::from(src.height)),
            t.width,
            t.height,
        );
        Ok(self.image_out(
            Capture {
                image: img,
                transform: t,
            },
            secure,
        ))
    }

    pub(super) fn web_zoom(
        &mut self,
        req: &ZoomRequest,
        w: &WindowInfo,
        page: PageId,
        t: &ImageTransform,
    ) -> CuResult<Reply> {
        let [x0, y0, x1, y1] = req.region;
        if x1 <= x0 || y1 <= y0 {
            return err(
                ErrorCode::BadRequest,
                "region is [x0, y0, x1, y1] with x1 > x0 and y1 > y0",
            );
        }
        if (w.frame.w - t.window_frame.w).abs() > 0.5 || (w.frame.h - t.window_frame.h).abs() > 0.5
        {
            return err(
                ErrorCode::StaleGeometry,
                format!("{} is older than the window's size", req.image),
            );
        }
        let snap = self.browser(page.0)?.snapshot(&page.1, w)?;
        let vp = snap.viewport;
        let region = t
            .region_to_window(Rect::new(x0, y0, x1 - x0, y1 - y0))
            .intersect(&vp.rect());
        if region.is_empty() {
            return err(ErrorCode::BadRequest, "the region is outside the page");
        }
        let src = self.on_page(&page, |p, c| p.screenshot(c))?;
        // The page image's pixels per window point: the display's full resolution.
        let device = f64::from(src.width) / vp.rect().w.max(1.0);
        let t2 = ImageTransform::fit(w.id, w.frame, region, device, geom::MAX_ZOOM_SIDE);
        let from = Rect::new(
            (region.x - vp.origin.x) * device,
            (region.y - vp.origin.y) * device,
            region.w * device,
            region.h * device,
        );
        let img = image::resample(&src, from, t2.width, t2.height);
        let secure = Self::secure_frames(&snap.nodes);
        let out = self.image_out(
            Capture {
                image: img,
                transform: t2,
            },
            &secure,
        );
        let t3 = self.image(&out.id)?;
        let text = format!(
            "zoom of {} · click in either image by its id\n{}",
            req.image,
            self.image_line(&out, &t3)
        );
        Ok(Reply {
            text,
            image: Some(out),
            ..Default::default()
        })
    }

    /// The element behind a page ref, after the checks of §4.3.
    fn web_resolve(
        &mut self,
        w: &WindowInfo,
        page: &PageId,
        r: u32,
        need_enabled: bool,
    ) -> CuResult<WebEl> {
        let refs = self
            .web_refs
            .get(&w.id)
            .ok_or_else(|| CuError::new(ErrorCode::StaleRef, "observe the page first"))?;
        let rec = refs.get(r).ok_or_else(|| {
            CuError::new(
                ErrorCode::StaleRef,
                format!("e{r} is not in the last observation"),
            )
        })?;
        if rec.generation != refs.generation {
            return err(
                ErrorCode::StaleRef,
                format!("e{r} is from before the page changed"),
            );
        }
        let (el, role, label) = (rec.element.clone(), rec.role.clone(), rec.label.clone());
        if el.is_dialog() {
            let open = self.on_page(page, |p, _| Ok(p.dialog.is_some()))?;
            if !open {
                return err(ErrorCode::StaleRef, format!("e{r}'s dialog is gone"));
            }
            return Ok(el);
        }
        if self.on_page(page, |p, _| Ok(p.dialog.is_some()))? {
            return err(
                ErrorCode::BadRequest,
                "the page waits on its dialog; answer it first (observe shows it)",
            );
        }
        // The page's root stands for the page; it has no node of its own to read.
        if el.node == 0 {
            return Ok(el);
        }
        let now = self
            .on_page(page, |_, c| web_page::read(c, &el))
            .map_err(|_| CuError::new(ErrorCode::StaleRef, format!("e{r} is gone")))?;
        if now.role != role || now.label != label {
            return err(
                ErrorCode::StaleRef,
                format!(
                    "e{r} is now a {} {:?}",
                    now.role,
                    now.label.unwrap_or_default()
                ),
            );
        }
        if need_enabled && !now.enabled {
            return err(ErrorCode::StaleRef, format!("e{r} is disabled"));
        }
        Ok(el)
    }

    /// A target's point in main-viewport CSS pixels, and in the window for the cursor and the
    /// log: an element is scrolled into view and must be what the page would hit there.
    fn web_point(
        &mut self,
        w: &WindowInfo,
        page: &PageId,
        vp: &Viewport,
        t: &Target,
        check_hit: bool,
    ) -> CuResult<WebPoint> {
        if let Some(r) = t.r#ref.as_deref() {
            let r = Self::ref_of(r)?;
            let el = self.web_resolve(w, page, r, true)?;
            if el.is_dialog() {
                return err(
                    ErrorCode::BadRequest,
                    "a dialog's buttons are pressed, not pointed at",
                );
            }
            let b = self.on_page(page, |p, c| {
                input::scroll_into_view(c, &el)?;
                p.css_box(c, &el)
            })?;
            let css = (b.x + b.w / 2.0, b.y + b.h / 2.0);
            if css.0 < 0.0 || css.1 < 0.0 || css.0 >= vp.css_w || css.1 >= vp.css_h {
                return err(
                    ErrorCode::NoSuchTarget,
                    format!("e{r} is outside the page's view even scrolled to"),
                );
            }
            if check_hit {
                let hit = self.on_page(page, |p, c| input::hit(c, p, &el, css))?;
                if let Err(on_top) = hit {
                    return err(
                        ErrorCode::Occluded,
                        format!("{on_top} is over e{r} there; close or scroll it away first"),
                    );
                }
            }
            let at = vp.to_window(css.0, css.1);
            let p0 = vp.to_window(b.x, b.y);
            let boxed = Rect::new(p0.x, p0.y, b.w * vp.zoom, b.h * vp.zoom);
            return Ok((css, (at, Some(boxed)), Some(el)));
        }
        let (Some(img), Some(x), Some(y)) = (t.image.as_deref(), t.x, t.y) else {
            // A scroll may name no place: the middle of the page.
            if !check_hit {
                let c = (vp.css_w / 2.0, vp.css_h / 2.0);
                return Ok((c, (vp.to_window(c.0, c.1), None), None));
            }
            return err(
                ErrorCode::BadRequest,
                "give a ref, or an image with x and y",
            );
        };
        let tr = self.image(img)?;
        if tr.window != w.id {
            return err(ErrorCode::BadRequest, format!("{img} shows another window"));
        }
        let p = match tr.to_window(x, y, w.frame) {
            Ok(p) => p,
            Err(MapError::StaleGeometry) => {
                return err(
                    ErrorCode::StaleGeometry,
                    format!("{img} is older than the window's size"),
                );
            }
            Err(MapError::OutsideImage) => {
                return err(ErrorCode::BadRequest, format!("{x},{y} is outside {img}"));
            }
        };
        let css = vp.to_css(p);
        if css.0 < 0.0 || css.1 < 0.0 || css.0 >= vp.css_w || css.1 >= vp.css_h {
            return err(
                ErrorCode::NoSuchTarget,
                format!("{x},{y} is on the browser's toolbar, not the page"),
            );
        }
        Ok((css, (p, None), None))
    }

    /// Answers the page's dialog through one of its virtual elements.
    fn web_answer(&mut self, page: &PageId, el: &WebEl, how: &str) -> CuResult<()> {
        let accept = match (el.node, how) {
            (DIALOG_ACCEPT, "press" | "accept" | "confirm") => true,
            (DIALOG_DISMISS, "press" | "dismiss" | "cancel") => false,
            (DIALOG_BOX | DIALOG_TEXT, "accept" | "confirm") => true,
            (DIALOG_BOX | DIALOG_TEXT, "dismiss" | "cancel") => false,
            _ => {
                return err(
                    ErrorCode::NoSuchAction,
                    "press the dialog's accept or dismiss button",
                );
            }
        };
        self.on_page(page, |p, c| p.answer(c, accept))
    }

    /// Refuses typing while a password field of the page has the focus.
    fn web_check_typing(&mut self, page: &PageId) -> CuResult<()> {
        let secure = self.on_page(page, |p, c| {
            let r = c.call(
                Some(&p.session),
                "Runtime.evaluate",
                serde_json::json!({"expression": "(() => { let a = document.activeElement; \
                    while (a && a.contentDocument && a.contentDocument.activeElement) a = a.contentDocument.activeElement; \
                    return !!a && a.type === 'password'; })()", "returnByValue": true}),
            )?;
            Ok(r["result"]["value"].as_bool().unwrap_or(false))
        })?;
        if secure {
            return err(ErrorCode::SecureField, "a password field has the focus");
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn web_act_one(
        &mut self,
        worker: &str,
        w: &WindowInfo,
        page: PageId,
        index: usize,
        action: &Action,
        cancel: &CancelToken,
        entered: Instant,
    ) -> CuResult<(ActionResult, bool)> {
        let user_before = self.desktop.user_focus();
        let (doc_before, opened_before, dialog_before) = self.on_page(&page, |p, _| {
            Ok((p.loader.clone(), p.opened.len(), p.dialog.is_some()))
        })?;
        let vp = self.on_page(&page, |p, c| p.viewport(c, w))?;
        if dialog_before
            && !matches!(
                action,
                Action::Click { .. }
                    | Action::Perform { .. }
                    | Action::SetValue { .. }
                    | Action::Wait { .. }
            )
        {
            return err(
                ErrorCode::BadRequest,
                "the page waits on its dialog; answer it with its accept or dismiss button",
            );
        }
        let mut out = Outcome::default();
        let start = Instant::now();
        match action {
            Action::Click {
                target,
                button,
                count,
                modifiers,
                ..
            } => {
                let mods = parse_mods(modifiers)?;
                let dialog_el = match target.r#ref.as_deref() {
                    Some(r) => {
                        let el = self.web_resolve(w, &page, Self::ref_of(r)?, true)?;
                        el.is_dialog().then_some(el)
                    }
                    None => None,
                };
                if let Some(el) = dialog_el {
                    self.web_answer(&page, &el, "press")?;
                    out.answered = true;
                } else {
                    let (css, aim, el) = self.web_point(w, &page, &vp, target, true)?;
                    if let Some(el) = &el {
                        out.before = self.on_page(&page, |_, c| web_page::read(c, el)).ok();
                    }
                    out.aim = Some(aim);
                    self.show_cursor(worker, w, out.aim, Gesture::Click);
                    self.on_page(&page, |p, c| input::click(c, p, css, *button, *count, mods))?;
                }
            }
            Action::SetValue { r#ref, text, .. } => {
                let el = self.web_resolve(w, &page, Self::ref_of(r#ref)?, true)?;
                if el.is_dialog() {
                    if el.node != DIALOG_TEXT {
                        return err(ErrorCode::NotSettable, "only a prompt's answer takes text");
                    }
                    self.on_page(&page, |p, _| {
                        if let Some(d) = p.dialog.as_mut() {
                            d.text = Some(text.clone());
                        }
                        Ok(())
                    })?;
                    out.answered = true;
                } else {
                    out.aim = self.ref_aim_web(w.id, r#ref);
                    self.show_cursor(worker, w, out.aim, Gesture::Type);
                    let node = self.on_page(&page, |_, c| web_page::read(c, &el))?;
                    if node.secure {
                        return err(ErrorCode::SecureField, "that is a password field");
                    }
                    if node.role == "popup" {
                        let label = self
                            .on_page(&page, |p, c| input::pick_option(c, p, &el, text, cancel))?;
                        out.option = Some((el, label));
                    } else {
                        self.on_page(&page, |p, c| input::replace_text(c, p, &el, text, cancel))?;
                        out.text = Some((el, text.clone()));
                    }
                }
            }
            Action::Type { text, r#ref, .. } => {
                if let Some(r) = r#ref.as_deref() {
                    let el = self.web_resolve(w, &page, Self::ref_of(r)?, true)?;
                    out.aim = self.ref_aim_web(w.id, r);
                    if self.on_page(&page, |_, c| web_page::read(c, &el))?.secure {
                        return err(ErrorCode::SecureField, "that is a password field");
                    }
                    self.on_page(&page, |_, c| input::focus(c, &el))?;
                }
                self.show_cursor(worker, w, out.aim, Gesture::Type);
                self.web_check_typing(&page)?;
                self.on_page(&page, |p, c| input::insert(c, p, text, cancel))?;
            }
            Action::Key { key, repeat, .. } => {
                let chord = Chord::parse(key)
                    .ok_or_else(|| CuError::new(ErrorCode::BadRequest, format!("bad key {key}")))?;
                if crate::cdp::keys::page_key(&chord).is_some_and(|k| k.text.is_some()) {
                    self.web_check_typing(&page)?;
                }
                for _ in 0..(*repeat).max(1) {
                    cancel.check()?;
                    self.on_page(&page, |p, c| input::key(c, p, &chord))?;
                }
            }
            Action::Scroll { target, dx, dy, .. } => {
                // The page itself (its root ref) scrolls at the middle of its view.
                let root = match target.r#ref.as_deref() {
                    Some(r) => self.web_resolve(w, &page, Self::ref_of(r)?, false)?.node == 0,
                    None => false,
                };
                let (css, aim) = if root {
                    let css = (vp.css_w / 2.0, vp.css_h / 2.0);
                    (css, (vp.to_window(css.0, css.1), None))
                } else {
                    let (css, aim, _) = self.web_point(w, &page, &vp, target, false)?;
                    (css, aim)
                };
                out.aim = Some((aim.0, None));
                self.show_cursor(worker, w, out.aim, Gesture::Scroll);
                self.on_page(&page, |p, c| input::wheel(c, p, css, *dx, *dy))?;
            }
            Action::Drag { from, to, .. } => {
                let (a, aim, _) = self.web_point(w, &page, &vp, from, true)?;
                let (b, to_aim, _) = self.web_point(w, &page, &vp, to, false)?;
                out.aim = Some((aim.0, None));
                let to_global = Point::new(w.frame.x + to_aim.0.x, w.frame.y + to_aim.0.y);
                self.show_cursor(worker, w, out.aim, Gesture::Drag { to: to_global });
                self.on_page(&page, |p, c| input::drag(c, p, a, b, cancel))?;
            }
            Action::Perform { r#ref, action, .. } => {
                let el = self.web_resolve(w, &page, Self::ref_of(r#ref)?, true)?;
                if el.is_dialog() {
                    self.web_answer(&page, &el, action)?;
                    out.answered = true;
                } else {
                    out.aim = self.ref_aim_web(w.id, r#ref);
                    self.show_cursor(worker, w, out.aim, Gesture::Press);
                    out.before = self.on_page(&page, |_, c| web_page::read(c, &el)).ok();
                    match action.as_str() {
                        "press" | "click" => {
                            let t = Target {
                                r#ref: Some(r#ref.clone()),
                                ..Default::default()
                            };
                            let (css, aim, _) = self.web_point(w, &page, &vp, &t, true)?;
                            out.aim = Some(aim);
                            self.on_page(&page, |p, c| {
                                input::click(
                                    c,
                                    p,
                                    css,
                                    crate::desktop::Button::Left,
                                    1,
                                    Mods::default(),
                                )
                            })?;
                        }
                        "focus" => self.on_page(&page, |_, c| input::focus(c, &el))?,
                        "scroll-to-visible" => {
                            self.on_page(&page, |_, c| input::scroll_into_view(c, &el))?;
                        }
                        other => {
                            return err(
                                ErrorCode::NoSuchAction,
                                format!(
                                    "a page element takes press, focus or scroll-to-visible, not {other}"
                                ),
                            );
                        }
                    }
                }
            }
            Action::Select {
                r#ref,
                start: from,
                length,
                ..
            } => {
                let el = self.web_resolve(w, &page, Self::ref_of(r#ref)?, true)?;
                out.aim = self.ref_aim_web(w.id, r#ref);
                self.show_cursor(worker, w, out.aim, Gesture::Press);
                if self.on_page(&page, |_, c| web_page::read(c, &el))?.secure {
                    return err(ErrorCode::SecureField, "that is a password field");
                }
                self.on_page(&page, |_, c| input::select_range(c, &el, *from, *length))?;
            }
            Action::Wait { .. } => {}
            Action::Navigate { url, .. } => {
                self.on_page(&page, |p, c| p.navigate(c, url))?;
            }
            Action::Menu { .. } => {
                return err(
                    ErrorCode::Failed,
                    "a browser's menus are the app's, not the page's",
                );
            }
        }
        let dispatch = start.elapsed();
        let bound = match action {
            Action::Wait { timeout_ms, .. } => Duration::from_millis(*timeout_ms).min(MAX_WAIT),
            Action::Navigate { .. } => SETTLE_BOUND * 4,
            _ => SETTLE_BOUND,
        }
        .min(cancel.remaining());
        let (settled, effect_at) = match action.expect() {
            Some(e) => {
                let mut held = None;
                loop {
                    cancel.check()?;
                    if let Some(b) = self.web.browser(page.0) {
                        b.absorb();
                    }
                    if self.web_expect(w, &page, e) {
                        held = Some(start.elapsed());
                        break;
                    }
                    if start.elapsed() >= bound {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(8));
                }
                (held.is_some(), held)
            }
            None => {
                let settled = self
                    .browser(page.0)?
                    .settle(&page.1, start, QUIET, bound, || cancel.check())?;
                (settled, None)
            }
        };
        let settle_ms = start.elapsed();
        let mut status = Status::Done;
        let mut error = None;
        let mut fail = |code, why: String| {
            status = Status::Failed;
            error = Some(CuError::new(code, why));
            Effect::NoChange
        };
        let effect = if action.expect().is_some() {
            if effect_at.is_some() {
                Effect::Confirmed
            } else {
                fail(
                    ErrorCode::Failed,
                    "the expected change didn't happen".into(),
                )
            }
        } else if out.answered {
            Effect::Confirmed
        } else if let Some((el, want)) = &out.text {
            let now = self
                .on_page(&page, |_, c| input::text_of(c, el))
                .unwrap_or_default();
            if now == *want {
                Effect::Confirmed
            } else {
                fail(ErrorCode::NotSettable, format!("the value is now {now:?}"))
            }
        } else if let Some((el, want)) = &out.option {
            let now = self
                .on_page(&page, |_, c| {
                    web_page::call_on(
                        c,
                        el,
                        "function(){ const o = this.options[this.selectedIndex]; return o ? o.label : ''; }",
                        &[],
                    )
                })
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            if now == *want {
                Effect::Confirmed
            } else {
                fail(
                    ErrorCode::NotSettable,
                    format!("the menu now shows {now:?}"),
                )
            }
        } else if let Some(before) = &out.before {
            match self.on_page(&page, |_, c| web_page::read(c, &before.element)) {
                Ok(a) if tree::render_line(&a) != tree::render_line(before) => Effect::Confirmed,
                Err(_) => Effect::Confirmed,
                Ok(_) => Effect::Unverified,
            }
        } else {
            Effect::Unverified
        };
        // A new document, a new tab or a dialog: the rest of the batch aims at what is gone.
        let mut notes = Vec::new();
        let (doc_after, opened, dialog_after, url) = self.on_page(&page, |p, _| {
            Ok((
                p.loader.clone(),
                p.opened.split_off(opened_before.min(p.opened.len())),
                p.dialog.clone(),
                p.url.clone(),
            ))
        })?;
        let mut navigated = false;
        if doc_after != doc_before {
            navigated = true;
            notes.push(format!(
                "the page is now {}",
                tree::quote(&url, tree::VALUE_CLIP)
            ));
            if let Some(refs) = self.web_refs.get_mut(&w.id) {
                refs.navigate();
            }
            self.web_docs.insert(w.id, doc_after);
        }
        if !opened.is_empty() {
            navigated = true;
            notes.push(format!("the page opened {} new tab(s)", opened.len()));
        }
        let dialog_closed = dialog_before && dialog_after.is_none();
        if let Some(d) = dialog_after.filter(|_| !dialog_before) {
            navigated = true;
            notes.push(format!(
                "the page opened a {}: {}",
                d.kind,
                tree::quote(&d.message, tree::VALUE_CLIP)
            ));
        }
        // The page behind it runs again; a prompt's text only filled the field.
        if dialog_closed {
            navigated = true;
        }
        let user_after = self.desktop.user_focus();
        if user_after != user_before {
            notes.push("the user's focus changed during the action".into());
        }
        let result = ActionResult {
            index,
            action: action.kind().into(),
            status,
            delivered: Some(Rung::Page),
            settled,
            effect: Some(effect),
            error,
            timings: Timings {
                checks_ms: ms(start - entered),
                dispatch_ms: ms(dispatch),
                effect_ms: effect_at.map(ms),
                settle_ms: ms(settle_ms),
            },
            notes,
        };
        let mut record = ActionRecord::new(worker, w, action, &result, user_after == user_before);
        (record.point, record.element_box) = out.aim.map_or((None, None), |(p, b)| (Some(p), b));
        self.records.push(record);
        Ok((result, navigated))
    }

    /// A page ref's last seen box and its centre.
    fn ref_aim_web(&self, window: u32, r: &str) -> Option<(Point, Option<Rect>)> {
        let f = self
            .web_refs
            .get(&window)?
            .get(tree::parse_ref(r)?)?
            .frame?;
        Some((f.center(), Some(f)))
    }

    pub(super) fn web_expect(&mut self, w: &WindowInfo, page: &PageId, e: &Expect) -> bool {
        let el_of = |this: &Self, r: &str| -> Option<WebEl> {
            Some(
                this.web_refs
                    .get(&w.id)?
                    .get(tree::parse_ref(r)?)?
                    .element
                    .clone(),
            )
        };
        let read = |this: &mut Self, r: &str| -> Option<RawNode<WebEl>> {
            let el = el_of(this, r)?;
            if el.is_dialog() {
                let d = this.on_page(page, |p, _| Ok(p.dialog.clone())).ok()??;
                let mut n = RawNode::new(el.clone(), 0, "dialog");
                n.value = d.text.or(Some(d.default_prompt));
                return Some(n);
            }
            this.on_page(page, |_, c| web_page::read(c, &el)).ok()
        };
        let shown = |n: &RawNode<WebEl>| n.value.clone().or_else(|| n.label.clone());
        match e {
            Expect::ValueEquals { r#ref, text } => {
                read(self, r#ref).is_some_and(|n| shown(&n).as_deref() == Some(text))
            }
            Expect::ValueContains { r#ref, text } => read(self, r#ref).is_some_and(|n| {
                shown(&n)
                    .as_deref()
                    .is_some_and(|v| v.contains(text.as_str()))
            }),
            Expect::Checked { r#ref, on } => read(self, r#ref).is_some_and(|n| match n.checked {
                Some(c) => matches!((c, on), (tree::Check::On, true) | (tree::Check::Off, false)),
                None => n.selected == *on,
            }),
            Expect::Gone { r#ref } => read(self, r#ref).is_none(),
            Expect::Focused { r#ref } => read(self, r#ref).is_some_and(|n| n.focused),
            Expect::TitleContains { text } => self
                .browser(page.0)
                .ok()
                .and_then(|b| b.snapshot(&page.1, w).ok())
                .is_some_and(|s| s.title.contains(text.as_str())),
            Expect::Appears { find } => {
                let needle = find.to_lowercase();
                self.browser(page.0)
                    .ok()
                    .and_then(|b| b.snapshot(&page.1, w).ok())
                    .is_some_and(|s| {
                        s.nodes
                            .iter()
                            .any(|n| tree::render_line(n).to_lowercase().contains(&needle))
                    })
            }
        }
    }
}
