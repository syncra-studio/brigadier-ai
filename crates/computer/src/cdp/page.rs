//! One tab: its sessions (the page and each out-of-process frame), what is in flight, its
//! dialog, the snapshot a model reads and the input that reaches it.
//!
//! Coordinates, worked out once here (measured on Chromium 155): a box model is in CSS pixels
//! of its frame's local root viewport, already scrolled. Frames in the page's own process share
//! the main frame's viewport; an out-of-process frame has its own, whose origin is its owner
//! element's content box in the parent's viewport. The main viewport maps to window points by
//! the page zoom, below the browser's toolbar: `window = viewport origin + css × zoom`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::conn::{Conn, Event};
use crate::desktop::WindowInfo;
use crate::error::{CuError, CuResult, ErrorCode, err};
use crate::geom::{Point, Rect};
use crate::tree::{Check, RawNode};

/// The session a dialog's virtual elements live in.
pub const DIALOG: &str = "dialog";
/// The dialog's virtual elements.
pub const DIALOG_BOX: i64 = 1;
pub const DIALOG_ACCEPT: i64 = 2;
pub const DIALOG_DISMISS: i64 = 3;
pub const DIALOG_TEXT: i64 = 4;

/// A page element: its renderer session and its DOM node.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WebEl {
    pub session: String,
    pub node: i64,
}

impl WebEl {
    pub fn is_dialog(&self) -> bool {
        self.session == DIALOG
    }
}

/// A JavaScript dialog the page opened; the page waits until it is answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    /// `alert`, `confirm`, `prompt` or `beforeunload`.
    pub kind: String,
    pub message: String,
    pub default_prompt: String,
    /// What a `prompt`'s field holds now, set through `set_value`.
    pub text: Option<String>,
}

/// How the main frame's viewport sits in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Its top left, in window points.
    pub origin: Point,
    /// Window points per CSS pixel.
    pub zoom: f64,
    /// Its size in CSS pixels.
    pub css_w: f64,
    pub css_h: f64,
}

impl Viewport {
    pub fn to_window(&self, x: f64, y: f64) -> Point {
        Point::new(self.origin.x + x * self.zoom, self.origin.y + y * self.zoom)
    }

    pub fn to_css(&self, p: Point) -> (f64, f64) {
        (
            (p.x - self.origin.x) / self.zoom,
            (p.y - self.origin.y) / self.zoom,
        )
    }

    /// The viewport in window points.
    pub fn rect(&self) -> Rect {
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.css_w * self.zoom,
            self.css_h * self.zoom,
        )
    }
}

/// A snapshot node with what actions need beyond the tree: the DOM element's tag.
pub type PageNode = RawNode<WebEl>;

/// What `observe` reads from a page.
pub struct Snapshot {
    pub nodes: Vec<PageNode>,
    pub viewport: Viewport,
    pub url: String,
    pub title: String,
    pub dialog: Option<Dialog>,
    /// The page's document, which changes on a navigation.
    pub loader: String,
}

/// An out-of-process frame's session.
#[derive(Debug, Clone)]
struct Child {
    session: String,
    /// The frame's id, which is also its target id.
    frame: String,
    /// The session of the frame that holds its owner element.
    parent: String,
}

pub struct Page {
    pub target: String,
    pub session: String,
    children: Vec<Child>,
    /// Requests in flight by id, which is the same in every session (an out-of-process
    /// frame's document starts in its parent's session and finishes in its own): the
    /// session that started each, when, and its URL.
    inflight: HashMap<String, (String, Instant, String)>,
    net_changed: Instant,
    pub loader: String,
    main_frame: String,
    pub url: String,
    pub dialog: Option<Dialog>,
    /// The isolated world that watches DOM changes, for the document it was made in.
    world: Option<(String, i64)>,
    /// Tabs this page opened since they were last reported.
    pub opened: Vec<String>,
    pub closed: bool,
}

/// Installs the DOM-change clock in an isolated world: the page's own scripts never see it.
/// How long a request may be open before it no longer holds the page unsettled.
const LONG_REQUEST: Duration = Duration::from_secs(1);

const QUIET_INSTALL: &str = "(() => { if (globalThis.__brigadierLast !== undefined) return true; \
globalThis.__brigadierLast = performance.now(); \
new MutationObserver(() => { globalThis.__brigadierLast = performance.now(); }) \
.observe(document, {subtree: true, childList: true, attributes: true, characterData: true}); \
return true; })()";
const QUIET_READ: &str = "performance.now() - globalThis.__brigadierLast";

impl Page {
    pub fn attach(conn: &mut Conn, target: &str) -> CuResult<Self> {
        let r = conn.call(
            None,
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
        )?;
        let session = r["sessionId"]
            .as_str()
            .ok_or_else(|| CuError::new(ErrorCode::Failed, "the page gave no session"))?
            .to_owned();
        let s = Some(session.as_str());
        conn.call(s, "Page.enable", json!({}))?;
        conn.call(s, "Network.enable", json!({}))?;
        conn.call(
            s,
            "Target.setAutoAttach",
            json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true}),
        )?;
        let tree = conn.call(s, "Page.getFrameTree", json!({}))?;
        let frame = &tree["frameTree"]["frame"];
        Ok(Self {
            target: target.to_owned(),
            session,
            children: Vec::new(),
            inflight: HashMap::new(),
            net_changed: Instant::now(),
            loader: frame["loaderId"].as_str().unwrap_or_default().to_owned(),
            main_frame: frame["id"].as_str().unwrap_or_default().to_owned(),
            url: frame["url"].as_str().unwrap_or_default().to_owned(),
            dialog: None,
            world: None,
            opened: Vec::new(),
            closed: false,
        })
    }

    /// Whether an event from `session` belongs to this page.
    pub fn owns(&self, session: Option<&str>) -> bool {
        session.is_some_and(|s| s == self.session || self.children.iter().any(|c| c.session == s))
    }

    pub fn apply(&mut self, conn: &mut Conn, e: &Event) {
        let session = e.session.clone().unwrap_or_default();
        let p = &e.params;
        match e.method.as_str() {
            "Target.attachedToTarget" => {
                let info = &p["targetInfo"];
                if let (Some(child), Some(frame)) =
                    (p["sessionId"].as_str(), info["targetId"].as_str())
                    && info["type"] == "iframe"
                {
                    let s = Some(child);
                    let _ = conn.call(s, "Network.enable", json!({}));
                    let _ = conn.call(
                        s,
                        "Target.setAutoAttach",
                        json!({"autoAttach": true, "waitForDebuggerOnStart": false, "flatten": true}),
                    );
                    self.children.push(Child {
                        session: child.to_owned(),
                        frame: frame.to_owned(),
                        parent: session,
                    });
                }
            }
            "Target.detachedFromTarget" => {
                if let Some(s) = p["sessionId"].as_str() {
                    self.children.retain(|c| c.session != s);
                }
            }
            "Network.requestWillBeSent" => {
                if let Some(id) = p["requestId"].as_str() {
                    let url = p["request"]["url"].as_str().unwrap_or_default().to_owned();
                    self.inflight
                        .insert(id.to_owned(), (session.to_owned(), Instant::now(), url));
                    self.net_changed = Instant::now();
                }
            }
            "Network.loadingFinished" | "Network.loadingFailed" => {
                if let Some(id) = p["requestId"].as_str() {
                    self.inflight.remove(id);
                    self.net_changed = Instant::now();
                }
            }
            "Page.javascriptDialogOpening" => {
                let s = |k: &str| p[k].as_str().unwrap_or_default().to_owned();
                self.dialog = Some(Dialog {
                    kind: s("type"),
                    message: s("message"),
                    default_prompt: s("defaultPrompt"),
                    text: None,
                });
            }
            "Page.javascriptDialogClosed" if session == self.session => self.dialog = None,
            "Page.frameNavigated" if session == self.session => {
                let f = &p["frame"];
                if f.get("parentId").is_none() {
                    self.loader = f["loaderId"].as_str().unwrap_or_default().to_owned();
                    self.main_frame = f["id"].as_str().unwrap_or_default().to_owned();
                    self.url = f["url"].as_str().unwrap_or_default().to_owned();
                    self.world = None;
                    // A new document: whatever was in flight belonged to the old one.
                    self.inflight.retain(|_, (s, _, _)| *s != session);
                }
            }
            _ => {}
        }
    }

    /// How long the page has been quiet: no request in flight and no DOM change. Zero while
    /// something is in flight or the page can't be asked.
    pub fn quiet_for(&mut self, conn: &mut Conn) -> Duration {
        // A request open longer than a second is a long poll, a stream or a beacon the
        // browser never reports finished: waiting on it would never settle.
        let busy = self
            .inflight
            .values()
            .any(|(_, at, _)| at.elapsed() < LONG_REQUEST);
        if busy || self.dialog.is_some() {
            return Duration::ZERO;
        }
        let net = self.net_changed.elapsed();
        let dom = match self.dom_quiet(conn) {
            Some(d) => d,
            None => return Duration::ZERO,
        };
        net.min(dom)
    }

    fn dom_quiet(&mut self, conn: &mut Conn) -> Option<Duration> {
        let s = Some(self.session.as_str());
        let ctx = match &self.world {
            Some((loader, ctx)) if *loader == self.loader => *ctx,
            _ => {
                let r = conn
                    .call(
                        s,
                        "Page.createIsolatedWorld",
                        json!({"frameId": self.main_frame, "worldName": "brigadier-settle"}),
                    )
                    .ok()?;
                let ctx = r["executionContextId"].as_i64()?;
                conn.call(
                    s,
                    "Runtime.evaluate",
                    json!({"expression": QUIET_INSTALL, "contextId": ctx, "returnByValue": true}),
                )
                .ok()?;
                self.world = Some((self.loader.clone(), ctx));
                ctx
            }
        };
        let r = conn
            .call_within(
                s,
                "Runtime.evaluate",
                json!({"expression": QUIET_READ, "contextId": ctx, "returnByValue": true}),
                Duration::from_millis(500),
            )
            .ok();
        let ms = r.as_ref().and_then(|r| r["result"]["value"].as_f64());
        if ms.is_none() {
            self.world = None;
        }
        ms.map(|m| Duration::from_secs_f64(m.max(0.0) / 1000.0))
    }

    /// The viewport's place in window `w`: the toolbar is above it, the page fills the width.
    pub fn viewport(&self, conn: &mut Conn, w: &WindowInfo) -> CuResult<Viewport> {
        let m = conn.call(Some(&self.session), "Page.getLayoutMetrics", json!({}))?;
        let css = &m["cssLayoutViewport"];
        let css_w = css["clientWidth"].as_f64().unwrap_or(w.frame.w).max(1.0);
        let css_h = css["clientHeight"].as_f64().unwrap_or(w.frame.h).max(1.0);
        let zoom = m["cssVisualViewport"]["zoom"]
            .as_f64()
            .filter(|z| *z > 0.0)
            .unwrap_or(w.frame.w / css_w);
        let top = (w.frame.h - css_h * zoom).max(0.0);
        Ok(Viewport {
            origin: Point::new(0.0, top),
            zoom,
            css_w,
            css_h,
        })
    }

    /// Where a session's frame's viewport sits in the main viewport, in CSS pixels.
    fn frame_offset(&self, conn: &mut Conn, session: &str) -> CuResult<(f64, f64)> {
        let mut off = (0.0, 0.0);
        let mut s = session.to_owned();
        for _ in 0..16 {
            if s == self.session {
                return Ok(off);
            }
            let Some(c) = self.children.iter().find(|c| c.session == s).cloned() else {
                return err(ErrorCode::StaleRef, "that frame is gone");
            };
            let owner = conn.call(
                Some(&c.parent),
                "DOM.getFrameOwner",
                json!({"frameId": c.frame}),
            )?;
            let node = owner["backendNodeId"].as_i64().unwrap_or_default();
            let q = content_quad(conn, &c.parent, node)?;
            off.0 += q.x;
            off.1 += q.y;
            s = c.parent;
        }
        err(ErrorCode::Failed, "frames nested too deep")
    }

    /// An element's border box in main-viewport CSS pixels.
    pub fn css_box(&self, conn: &mut Conn, el: &WebEl) -> CuResult<Rect> {
        let r = border_quad(conn, &el.session, el.node)?;
        let (ox, oy) = self.frame_offset(conn, &el.session)?;
        Ok(Rect::new(r.x + ox, r.y + oy, r.w, r.h))
    }

    /// Reads the page. While a dialog is open the page's scripts are paused, so only the dialog
    /// is read: asking the page anything would wait on it.
    pub fn snapshot(&mut self, conn: &mut Conn, w: &WindowInfo) -> CuResult<Snapshot> {
        let viewport = self.viewport(conn, w)?;
        let title = conn
            .call(
                None,
                "Target.getTargetInfo",
                json!({"targetId": self.target}),
            )
            .ok()
            .and_then(|r| r["targetInfo"]["title"].as_str().map(str::to_owned))
            .unwrap_or_default();
        let mut root = RawNode::new(
            WebEl {
                session: self.session.clone(),
                node: 0,
            },
            0,
            "web",
        );
        root.label = Some(title.clone()).filter(|t| !t.is_empty());
        root.value = Some(self.url.clone());
        root.frame = Some(viewport.rect());
        let mut nodes = vec![root];
        if let Some(d) = &self.dialog {
            dialog_nodes(d, &viewport, &mut nodes);
        } else {
            let mut reader = Reader {
                conn,
                page: self,
                viewport,
                nodes: &mut nodes,
            };
            reader.frame(&self.session.clone(), None, 1)?;
        }
        Ok(Snapshot {
            nodes,
            viewport,
            url: self.url.clone(),
            title,
            dialog: self.dialog.clone(),
            loader: self.loader.clone(),
        })
    }
}

/// The virtual elements of an open dialog: its box, its field for a prompt, and its buttons.
fn dialog_nodes(d: &Dialog, v: &Viewport, out: &mut Vec<PageNode>) {
    let el = |node| WebEl {
        session: DIALOG.into(),
        node,
    };
    let mid = v.rect();
    let mut b = RawNode::new(el(DIALOG_BOX), 1, "dialog");
    b.label = Some(format!("{} from the page: {}", d.kind, d.message));
    b.frame = Some(Rect::new(
        mid.x + mid.w / 2.0 - 200.0,
        mid.y + 20.0,
        400.0,
        140.0,
    ));
    out.push(b);
    if d.kind == "prompt" {
        let mut t = RawNode::new(el(DIALOG_TEXT), 2, "textfield");
        t.value = Some(d.text.clone().unwrap_or_else(|| d.default_prompt.clone()));
        t.label = Some("answer".into());
        out.push(t);
    }
    let mut ok = RawNode::new(el(DIALOG_ACCEPT), 2, "button");
    ok.label = Some(if d.kind == "alert" { "OK" } else { "accept" }.into());
    out.push(ok);
    if d.kind != "alert" {
        let mut no = RawNode::new(el(DIALOG_DISMISS), 2, "button");
        no.label = Some("dismiss".into());
        out.push(no);
    }
}

/// Walks the accessibility trees of a page's frames into one pre-order list.
struct Reader<'a> {
    conn: &'a mut Conn,
    page: &'a Page,
    viewport: Viewport,
    nodes: &'a mut Vec<PageNode>,
}

/// One accessibility node, as the protocol gives it.
struct AxNode {
    ignored: bool,
    role: String,
    name: String,
    value: Option<String>,
    props: HashMap<String, Value>,
    children: Vec<String>,
    backend: Option<i64>,
}

fn ax_nodes(r: &Value) -> (Option<String>, HashMap<String, AxNode>) {
    let mut out = HashMap::new();
    let mut root = None;
    for n in r["nodes"].as_array().into_iter().flatten() {
        let id = n["nodeId"].as_str().unwrap_or_default().to_owned();
        if root.is_none() && n.get("parentId").is_none() {
            root = Some(id.clone());
        }
        let s = |v: &Value| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        };
        let props = n["properties"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p["name"].as_str()?.to_owned(), p["value"]["value"].clone())))
            .collect();
        out.insert(
            id,
            AxNode {
                ignored: n["ignored"].as_bool().unwrap_or(false),
                role: n["role"]["value"].as_str().unwrap_or_default().to_owned(),
                name: s(&n["name"]["value"]).unwrap_or_default(),
                value: s(&n["value"]["value"]),
                props,
                children: n["childIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.as_str().map(str::to_owned))
                    .collect(),
                backend: n["backendDOMNodeId"].as_i64(),
            },
        );
    }
    (root, out)
}

/// The neutral role of a page element, or `None` for nodes that only hold text runs.
fn page_role(role: &str, tag: Option<&str>) -> Option<&'static str> {
    if tag == Some("CANVAS") {
        return Some("canvas");
    }
    Some(match role {
        "button" => "button",
        "link" => "link",
        "checkbox" | "switch" => "checkbox",
        "radio" => "radio",
        "textbox" => {
            if tag == Some("PASSWORD") {
                "secure-field"
            } else if tag == Some("TEXTAREA") {
                "text-area"
            } else {
                "textfield"
            }
        }
        "searchbox" => "search-field",
        "combobox" => {
            if tag == Some("SELECT") {
                "popup"
            } else {
                "combo"
            }
        }
        "listbox" => "list",
        "option" | "MenuListOption" => "menu-item",
        "MenuListPopup" => "menu",
        "slider" => "slider",
        "spinbutton" => "stepper",
        "tab" => "tab",
        "tablist" => "tabs",
        "tabpanel" => "group",
        "table" | "grid" | "treegrid" => "table",
        "row" => "row",
        "cell" | "gridcell" | "columnheader" | "rowheader" => "cell",
        "list" => "list",
        "listitem" => "list-item",
        "heading" => "heading",
        "image" | "img" => "image",
        "dialog" | "alertdialog" => "dialog",
        "menu" | "menubar" => "menu",
        "menuitem" | "menuitemcheckbox" | "menuitemradio" => "menu-item",
        "progressbar" => "progress",
        "Iframe" | "IframePresentational" => "frame",
        "RootWebArea" | "WebArea" => "web",
        "StaticText" => "text",
        "LabelText" => "text",
        "InlineTextBox" | "LineBreak" => return None,
        "navigation" | "main" | "banner" | "contentinfo" | "complementary" | "region" | "form"
        | "search" | "article" | "section" | "group" | "toolbar" => "group",
        _ => "group",
    })
}

/// Roles worth a box: what a model clicks or reads a place for.
fn wants_box(role: &str) -> bool {
    !matches!(
        role,
        "text" | "group" | "web" | "list-item" | "menu" | "menu-item"
    )
}

impl Reader<'_> {
    /// Reads one frame's tree (the session's main document when `frame` is `None`) into the list
    /// at `depth`.
    fn frame(&mut self, session: &str, frame: Option<&str>, depth: u16) -> CuResult<()> {
        let params = match frame {
            Some(f) => json!({"frameId": f}),
            None => json!({}),
        };
        let r = self
            .conn
            .call(Some(session), "Accessibility.getFullAXTree", params)?;
        let (root, ax) = ax_nodes(&r);
        let tags = self.tags(session);
        let offset = self
            .page
            .frame_offset(self.conn, session)
            .unwrap_or((0.0, 0.0));
        // The frames this session holds below this one, by their owner element.
        let local = self.local_frames(session);
        let Some(root) = root else {
            return Ok(());
        };
        // The document node itself is the page or frame line already: start at its children.
        let start: Vec<String> = ax
            .get(&root)
            .map(|n| n.children.clone())
            .unwrap_or_default();
        let mut stack: Vec<(String, u16, Option<String>)> =
            start.into_iter().rev().map(|c| (c, depth, None)).collect();
        while let Some((id, d, parent_name)) = stack.pop() {
            let Some(n) = ax.get(&id) else { continue };
            let tag = n.backend.and_then(|b| tags.get(&b)).map(String::as_str);
            let role = if n.ignored && tag != Some("CANVAS") {
                None
            } else {
                page_role(&n.role, tag)
            };
            let mut child_depth = d;
            let mut name_here = parent_name.clone();
            if let (Some(role), Some(backend)) = (role, n.backend) {
                // A text run that repeats its parent's name adds nothing.
                let echo = role == "text" && parent_name.as_deref() == Some(n.name.as_str());
                if !echo {
                    let node = self.node(session, backend, role, n, tag, offset, d);
                    self.nodes.push(node);
                    child_depth = d + 1;
                    if !n.name.is_empty() {
                        name_here = Some(n.name.clone());
                    }
                    // A frame's document hangs under its owner: in this process, or in another.
                    if role == "frame" {
                        if let Some(f) = local.get(&backend) {
                            let _ = self.frame(session, Some(f), d + 1);
                        } else if let Some(c) = self.oopif_owned_by(session, backend) {
                            let _ = self.frame(&c, None, d + 1);
                        }
                    }
                }
            }
            for c in n.children.iter().rev() {
                stack.push((c.clone(), child_depth, name_here.clone()));
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn node(
        &mut self,
        session: &str,
        backend: i64,
        role: &str,
        n: &AxNode,
        tag: Option<&str>,
        offset: (f64, f64),
        depth: u16,
    ) -> PageNode {
        let el = WebEl {
            session: session.to_owned(),
            node: backend,
        };
        let mut out = RawNode::new(el, depth, role);
        let prop_bool = |k: &str| n.props.get(k).and_then(Value::as_bool);
        let prop_str = |k: &str| match n.props.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Bool(b)) => Some(b.to_string()),
            _ => None,
        };
        out.secure = tag == Some("PASSWORD");
        out.label = Some(n.name.clone()).filter(|s| !s.trim().is_empty());
        if role == "text" {
            out.label = out.label.or(n.value.clone());
        } else if !out.secure {
            out.value = n.value.clone().filter(|v| !v.is_empty());
        }
        out.enabled = !prop_bool("disabled").unwrap_or(false);
        out.focused = prop_bool("focused").unwrap_or(false);
        out.selected = prop_bool("selected").unwrap_or(false);
        out.expanded = prop_bool("expanded").filter(|e| *e);
        out.checked = prop_str("checked").map(|c| match c.as_str() {
            "true" => Check::On,
            "mixed" => Check::Mixed,
            _ => Check::Off,
        });
        if matches!(role, "checkbox" | "radio") && out.checked.is_none() {
            out.checked = Some(Check::Off);
        }
        if wants_box(role)
            && let Ok(q) = border_quad(self.conn, session, backend)
        {
            let p = self.viewport.to_window(q.x + offset.0, q.y + offset.1);
            out.frame = Some(Rect::new(
                p.x,
                p.y,
                q.w * self.viewport.zoom,
                q.h * self.viewport.zoom,
            ));
        }
        out
    }

    /// Element tags by DOM node, for the cases accessibility doesn't tell: a password field, a
    /// `<select>`, a canvas.
    fn tags(&mut self, session: &str) -> HashMap<i64, String> {
        let mut out = HashMap::new();
        let Ok(doc) = self.conn.call(
            Some(session),
            "DOM.getDocument",
            json!({"depth": -1, "pierce": true}),
        ) else {
            return out;
        };
        let mut stack = vec![&doc["root"]];
        while let Some(n) = stack.pop() {
            if let (Some(b), Some(name)) = (n["backendNodeId"].as_i64(), n["nodeName"].as_str()) {
                let password = name == "INPUT"
                    && n["attributes"].as_array().is_some_and(|a| {
                        a.chunks(2)
                            .any(|kv| kv[0] == "type" && kv.get(1).is_some_and(|v| v == "password"))
                    });
                out.insert(
                    b,
                    if password {
                        "PASSWORD".into()
                    } else {
                        name.to_owned()
                    },
                );
            }
            for k in ["children", "shadowRoots"] {
                if let Some(a) = n[k].as_array() {
                    stack.extend(a.iter());
                }
            }
            if n["contentDocument"].is_object() {
                stack.push(&n["contentDocument"]);
            }
        }
        out
    }

    /// The child frames this session renders itself, by the owner element's DOM node.
    fn local_frames(&mut self, session: &str) -> HashMap<i64, String> {
        let mut out = HashMap::new();
        let Ok(t) = self
            .conn
            .call(Some(session), "Page.getFrameTree", json!({}))
        else {
            return out;
        };
        let mut ids = Vec::new();
        let mut stack = vec![&t["frameTree"]];
        while let Some(f) = stack.pop() {
            let id = f["frame"]["id"].as_str().unwrap_or_default();
            if f["frame"]["parentId"].is_string() {
                ids.push(id.to_owned());
            }
            if let Some(c) = f["childFrames"].as_array() {
                stack.extend(c.iter());
            }
        }
        for id in ids {
            if self.page.children.iter().any(|c| c.frame == id) {
                continue;
            }
            if let Ok(o) =
                self.conn
                    .call(Some(session), "DOM.getFrameOwner", json!({"frameId": id}))
                && let Some(b) = o["backendNodeId"].as_i64()
            {
                out.insert(b, id);
            }
        }
        out
    }

    /// The out-of-process frame whose owner is `backend` in `session`.
    fn oopif_owned_by(&mut self, session: &str, backend: i64) -> Option<String> {
        let kids: Vec<Child> = self
            .page
            .children
            .iter()
            .filter(|c| c.parent == session)
            .cloned()
            .collect();
        for c in kids {
            if let Ok(o) = self.conn.call(
                Some(session),
                "DOM.getFrameOwner",
                json!({"frameId": c.frame}),
            ) && o["backendNodeId"].as_i64() == Some(backend)
            {
                return Some(c.session);
            }
        }
        None
    }
}

fn quad_rect(q: &Value) -> Option<Rect> {
    let a: Vec<f64> = q.as_array()?.iter().filter_map(Value::as_f64).collect();
    if a.len() < 8 {
        return None;
    }
    let xs = [a[0], a[2], a[4], a[6]];
    let ys = [a[1], a[3], a[5], a[7]];
    let (x0, x1) = (
        xs.iter().copied().fold(f64::MAX, f64::min),
        xs.iter().copied().fold(f64::MIN, f64::max),
    );
    let (y0, y1) = (
        ys.iter().copied().fold(f64::MAX, f64::min),
        ys.iter().copied().fold(f64::MIN, f64::max),
    );
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

fn box_model(conn: &mut Conn, session: &str, node: i64) -> CuResult<Value> {
    let r = conn.call(
        Some(session),
        "DOM.getBoxModel",
        json!({"backendNodeId": node}),
    )?;
    Ok(r["model"].clone())
}

/// A node's border box in its frame's viewport, CSS pixels.
pub fn border_quad(conn: &mut Conn, session: &str, node: i64) -> CuResult<Rect> {
    quad_rect(&box_model(conn, session, node)?["border"])
        .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, "the element has no box"))
}

/// A node's content box in its frame's viewport, CSS pixels.
pub fn content_quad(conn: &mut Conn, session: &str, node: i64) -> CuResult<Rect> {
    quad_rect(&box_model(conn, session, node)?["content"])
        .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, "the element has no box"))
}

/// Calls `function` on a page element with `args` (JSON values) and returns its value.
pub fn call_on(conn: &mut Conn, el: &WebEl, function: &str, args: &[Value]) -> CuResult<Value> {
    let s = Some(el.session.as_str());
    let r = conn.call(s, "DOM.resolveNode", json!({"backendNodeId": el.node}))?;
    let object = r["object"]["objectId"]
        .as_str()
        .ok_or_else(|| CuError::new(ErrorCode::StaleRef, "the element is gone"))?
        .to_owned();
    let args: Vec<Value> = args.iter().map(|v| json!({"value": v})).collect();
    let out = conn.call(
        s,
        "Runtime.callFunctionOn",
        json!({"objectId": object, "functionDeclaration": function, "arguments": args,
               "returnByValue": true, "awaitPromise": false}),
    );
    let _ = conn.call(s, "Runtime.releaseObject", json!({"objectId": object}));
    let out = out?;
    if let Some(e) = out.get("exceptionDetails") {
        return err(
            ErrorCode::Failed,
            format!(
                "the page threw: {}",
                e["exception"]["description"].as_str().unwrap_or("an error")
            ),
        );
    }
    Ok(super::by_value(&out))
}

/// One element read fresh: its role, name, value and states, as a snapshot would show it.
pub fn read(conn: &mut Conn, el: &WebEl) -> CuResult<PageNode> {
    let r = conn.call(
        Some(&el.session),
        "Accessibility.getPartialAXTree",
        json!({"backendNodeId": el.node, "fetchRelatives": false}),
    )?;
    let (_, ax) = ax_nodes(&r);
    let n = ax
        .values()
        .find(|n| n.backend == Some(el.node))
        .ok_or_else(|| CuError::new(ErrorCode::StaleRef, "the element is gone"))?;
    let tag = call_on(
        conn,
        el,
        "function(){return this.nodeName === 'INPUT' && this.type === 'password' ? 'PASSWORD' : this.nodeName}",
        &[],
    )
    .ok()
    .and_then(|v| v.as_str().map(str::to_owned));
    let role = page_role(&n.role, tag.as_deref()).unwrap_or("group");
    let mut out = RawNode::new(el.clone(), 0, role);
    out.label = Some(n.name.clone()).filter(|s| !s.trim().is_empty());
    out.secure = tag.as_deref() == Some("PASSWORD");
    if role == "text" {
        out.label = out.label.or(n.value.clone());
    } else if !out.secure {
        out.value = n.value.clone().filter(|v| !v.is_empty());
    }
    let b = |k: &str| n.props.get(k).and_then(Value::as_bool);
    out.enabled = !b("disabled").unwrap_or(false);
    out.focused = b("focused").unwrap_or(false);
    out.selected = b("selected").unwrap_or(false);
    out.checked = n.props.get("checked").map(|c| match c {
        Value::String(s) if s == "true" => Check::On,
        Value::String(s) if s == "mixed" => Check::Mixed,
        Value::Bool(true) => Check::On,
        _ => Check::Off,
    });
    if matches!(role, "checkbox" | "radio") && out.checked.is_none() {
        out.checked = Some(Check::Off);
    }
    Ok(out)
}

impl Page {
    /// The out-of-process frames from the page down to `session`: each frame's id and session.
    pub fn chain_to(&self, session: &str) -> CuResult<Vec<(String, String)>> {
        let mut out = Vec::new();
        let mut s = session.to_owned();
        for _ in 0..16 {
            if s == self.session {
                out.reverse();
                return Ok(out);
            }
            let c = self
                .children
                .iter()
                .find(|c| c.session == s)
                .ok_or_else(|| CuError::new(ErrorCode::StaleRef, "that frame is gone"))?;
            out.push((c.frame.clone(), c.session.clone()));
            s = c.parent.clone();
        }
        err(ErrorCode::Failed, "frames nested too deep")
    }

    /// The viewport's pixels, at the display's full resolution, without the window coming to
    /// the front or even being on screen.
    pub fn screenshot(&self, conn: &mut Conn) -> CuResult<crate::redact::Rgba> {
        let r = conn.call(
            Some(&self.session),
            "Page.captureScreenshot",
            json!({"format": "png", "fromSurface": true, "captureBeyondViewport": false, "optimizeForSpeed": true}),
        )?;
        let data = r["data"]
            .as_str()
            .ok_or_else(|| CuError::new(ErrorCode::Failed, "the browser sent no image"))?;
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD
            .decode(data)
            .map_err(|e| CuError::new(ErrorCode::Failed, format!("the page image: {e}")))?;
        super::image::decode_png(&png)
    }

    /// Answers the open dialog.
    pub fn answer(&mut self, conn: &mut Conn, accept: bool) -> CuResult<()> {
        let Some(d) = self.dialog.clone() else {
            return err(ErrorCode::StaleRef, "the dialog is gone");
        };
        let mut params = json!({"accept": accept});
        if accept && d.kind == "prompt" {
            params["promptText"] = json!(d.text.unwrap_or(d.default_prompt));
        }
        conn.call(Some(&self.session), "Page.handleJavaScriptDialog", params)?;
        self.dialog = None;
        Ok(())
    }

    /// Loads a URL in this tab.
    pub fn navigate(&mut self, conn: &mut Conn, url: &str) -> CuResult<()> {
        let r = conn.call(Some(&self.session), "Page.navigate", json!({"url": url}))?;
        if let Some(e) = r["errorText"].as_str() {
            return err(ErrorCode::Failed, format!("the page didn't load: {e}"));
        }
        if let Some(l) = r["loaderId"].as_str() {
            self.loader = l.to_owned();
            self.world = None;
        }
        Ok(())
    }
}
