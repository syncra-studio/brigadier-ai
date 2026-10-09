//! The accessibility side: reading a window's elements in few calls, mapping roles to the
//! platform-neutral ones, and element actions.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::ptr::NonNull;

use objc2_application_services::{
    AXCopyMultipleAttributeOptions, AXError, AXUIElement, AXValue, AXValueType,
};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRange, CFRetained, CFString, CFType, CGPoint, CGSize,
};

use crate::error::{CuError, CuResult, ErrorCode, err};
use crate::geom::{Point, Rect};
use crate::tree::{Check, RawNode};

/// An accessibility element. Equal when the system says they are the same element.
#[derive(Clone)]
pub struct AxEl(pub CFRetained<AXUIElement>);

impl PartialEq for AxEl {
    fn eq(&self, o: &Self) -> bool {
        let a: &CFType = &self.0;
        let b: &CFType = &o.0;
        a == b
    }
}
impl Eq for AxEl {}
impl Hash for AxEl {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let a: &CFType = &self.0;
        a.hash(h);
    }
}
impl std::fmt::Debug for AxEl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AxEl({:p})", CFRetained::as_ptr(&self.0))
    }
}

impl AxEl {
    pub fn app(pid: i32) -> Self {
        // SAFETY: creating an application element has no preconditions.
        let el = unsafe { AXUIElement::new_application(pid) };
        // A hung app must never stall the engine (§4.1).
        // SAFETY: `el` is a live element.
        unsafe { el.set_messaging_timeout(1.0) };
        Self(el)
    }

    /// Element `id` of `pid` by remote token (see `Private::ax_remote_element`).
    pub fn remote(pid: i32, id: u64) -> Option<Self> {
        let el = super::private::Private::get().ax_remote_element(pid, id)?;
        // SAFETY: `el` is a live element.
        unsafe { el.set_messaging_timeout(1.0) };
        Some(Self(el))
    }

    pub fn attr(&self, name: &'static str) -> Result<CFRetained<CFType>, AXError> {
        let key = CFString::from_static_str(name);
        let mut out: *const CFType = std::ptr::null();
        // SAFETY: `out` is a valid out pointer; a non-null result is +1 retained.
        let e = unsafe { self.0.copy_attribute_value(&key, NonNull::from(&mut out)) };
        if e != AXError::Success {
            return Err(e);
        }
        NonNull::new(out.cast_mut())
            .map(|p| unsafe { CFRetained::from_raw(p) })
            .ok_or(AXError::NoValue)
    }

    pub fn string(&self, name: &'static str) -> Option<String> {
        self.attr(name)
            .ok()
            .and_then(|v| v.downcast::<CFString>().ok())
            .map(|s| s.to_string())
    }

    pub fn element(&self, name: &'static str) -> Option<AxEl> {
        self.attr(name)
            .ok()
            .and_then(|v| v.downcast::<AXUIElement>().ok())
            .map(AxEl)
    }

    pub fn elements(&self, name: &'static str) -> Vec<AxEl> {
        let Ok(v) = self.attr(name) else {
            return Vec::new();
        };
        let Ok(arr) = v.downcast::<CFArray>() else {
            return Vec::new();
        };
        // SAFETY: an accessibility element list holds CF objects.
        let arr: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(arr) };
        arr.iter()
            .filter_map(|x| x.downcast::<AXUIElement>().ok())
            .map(AxEl)
            .collect()
    }

    /// The element's top left in global points, as accessibility reports it.
    pub fn position(&self) -> Option<Point> {
        let p: CGPoint = self
            .attr("AXPosition")
            .ok()
            .and_then(|v| ax_value(v, AXValueType::CGPoint))?;
        Some(Point::new(p.x, p.y))
    }

    pub fn bool(&self, name: &'static str) -> Option<bool> {
        self.attr(name).ok().and_then(as_bool)
    }

    pub fn set(&self, name: &'static str, value: &CFType) -> CuResult<()> {
        let key = CFString::from_static_str(name);
        // SAFETY: both arguments are live CF objects.
        check(unsafe { self.0.set_attribute_value(&key, value) }, name)
    }

    /// Sets a range attribute (`AXSelectedTextRange`), in characters.
    pub fn set_range(&self, name: &'static str, start: usize, length: usize) -> CuResult<()> {
        let range = CFRange {
            location: start as isize,
            length: length as isize,
        };
        // SAFETY: `range` is a CFRange, the layout the CFRange type tag names.
        let v = unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&range).cast()) }
            .ok_or_else(|| CuError::new(ErrorCode::Failed, "couldn't make a range value"))?;
        self.set(name, &v)
    }

    /// A range attribute, in characters.
    pub fn range(&self, name: &'static str) -> Option<(usize, usize)> {
        let v = self.attr(name).ok()?.downcast::<AXValue>().ok()?;
        let mut r = CFRange {
            location: 0,
            length: 0,
        };
        // SAFETY: `r` is a CFRange, checked against the value's type tag first.
        let read = unsafe {
            v.r#type() == AXValueType::CFRange
                && v.value(AXValueType::CFRange, NonNull::from(&mut r).cast())
        };
        if !read {
            return None;
        }
        Some((
            usize::try_from(r.location).ok()?,
            usize::try_from(r.length).ok()?,
        ))
    }

    pub fn settable(&self, name: &'static str) -> bool {
        let key = CFString::from_static_str(name);
        let mut out = 0u8;
        // SAFETY: `out` is a valid out pointer (a Boolean).
        let e = unsafe {
            self.0
                .is_attribute_settable(&key, NonNull::from(&mut out).cast())
        };
        e == AXError::Success && out != 0
    }

    pub fn perform(&self, action: &str) -> CuResult<()> {
        let a = CFString::from_str(action);
        // SAFETY: a live element and action name.
        check(unsafe { self.0.perform_action(&a) }, action)
    }

    /// Performs an action, waiting at most `wait` for the app's reply. A reply still pending
    /// then is left to its thread: the action was delivered, and an error it may bring shows
    /// up as no effect.
    pub fn perform_bounded(&self, action: &str, wait: std::time::Duration) -> CuResult<()> {
        struct Sendable(AxEl);
        // SAFETY: accessibility elements are immutable references to another process's
        // objects; the accessibility API may be called from any thread.
        unsafe impl Send for Sendable {}
        let el = Sendable(self.clone());
        let action = action.to_owned();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ax-perform".into())
            .spawn(move || {
                let el = el;
                let _ = tx.send(el.0.perform(&action));
            })
            .map_err(|e| {
                CuError::new(ErrorCode::Failed, format!("couldn't start a thread: {e}"))
            })?;
        match rx.recv_timeout(wait) {
            Ok(r) => r,
            Err(_) => Ok(()),
        }
    }

    pub fn action_names(&self) -> Vec<String> {
        let mut out: *const CFArray = std::ptr::null();
        // SAFETY: `out` is a valid out pointer; a non-null result is +1 retained.
        let e = unsafe { self.0.copy_action_names(NonNull::from(&mut out)) };
        if e != AXError::Success {
            return Vec::new();
        }
        let Some(p) = NonNull::new(out.cast_mut()) else {
            return Vec::new();
        };
        // SAFETY: the action list holds CFStrings.
        let arr: CFRetained<CFArray<CFString>> =
            unsafe { CFRetained::cast_unchecked(CFRetained::from_raw(p)) };
        arr.iter().map(|s| s.to_string()).collect()
    }

    pub fn window_id(&self) -> Option<u32> {
        super::private::Private::get().ax_window_id(&self.0)
    }
}

/// Turns an accessibility error into an engine error.
pub fn check(e: AXError, what: &str) -> CuResult<()> {
    match e.0 {
        0 => Ok(()),
        -25202 => err(ErrorCode::StaleRef, format!("{what}: the element is gone")),
        -25204 => err(
            ErrorCode::AppNotResponding,
            format!("{what}: the app didn't answer"),
        ),
        -25205 => err(
            ErrorCode::NotSettable,
            format!("{what}: not supported by the element"),
        ),
        -25206 => err(
            ErrorCode::NoSuchAction,
            format!("{what}: not an action of the element"),
        ),
        -25211 => err(
            ErrorCode::PermissionMissing,
            "the Accessibility permission is missing",
        ),
        n => err(
            ErrorCode::Failed,
            format!("{what}: accessibility error {n}"),
        ),
    }
}

fn as_bool(v: CFRetained<CFType>) -> Option<bool> {
    match v.downcast::<CFBoolean>() {
        Ok(b) => Some(b.as_bool()),
        Err(v) => v
            .downcast::<CFNumber>()
            .ok()
            .and_then(|n| n.as_i64())
            .map(|n| n != 0),
    }
}

fn as_string(v: CFRetained<CFType>) -> Option<String> {
    match v.downcast::<CFString>() {
        Ok(s) => Some(s.to_string()),
        Err(v) => match v.downcast::<CFNumber>() {
            Ok(n) => match n.as_f64() {
                Some(f) if f.fract() == 0.0 && f.abs() < 1e15 => Some(format!("{}", f as i64)),
                Some(f) => Some(format!("{}", (f * 1000.0).round() / 1000.0)),
                None => None,
            },
            Err(v) => match v.downcast::<CFBoolean>() {
                Ok(b) => Some(b.as_bool().to_string()),
                Err(_) => None,
            },
        },
    }
}

fn ax_value<T: Default>(v: CFRetained<CFType>, ty: AXValueType) -> Option<T> {
    let v = v.downcast::<AXValue>().ok()?;
    let mut out = T::default();
    // SAFETY: `out` has the layout of `ty` (CGPoint, CGSize or CFRange).
    unsafe {
        if v.r#type() != ty {
            return None;
        }
        v.value(ty, NonNull::from(&mut out).cast()).then_some(out)
    }
}

/// The attributes fetched for every element, in one call per element.
const ATTRS: [&str; 13] = [
    "AXRole",
    "AXSubrole",
    "AXTitle",
    "AXDescription",
    "AXValue",
    "AXPosition",
    "AXSize",
    "AXEnabled",
    "AXFocused",
    "AXSelected",
    "AXExpanded",
    "AXChildren",
    "AXPlaceholderValue",
];

fn attr_names() -> CFRetained<CFArray<CFString>> {
    let names: Vec<CFRetained<CFString>> =
        ATTRS.iter().map(|a| CFString::from_static_str(a)).collect();
    CFArray::from_retained_objects(&names)
}

/// A platform-neutral role for an accessibility role and subrole.
pub fn neutral_role(role: &str, subrole: Option<&str>) -> String {
    match subrole {
        Some("AXSecureTextField") => return "secure-field".into(),
        Some("AXSearchField") => return "search-field".into(),
        Some("AXCloseButton")
        | Some("AXMinimizeButton")
        | Some("AXZoomButton")
        | Some("AXFullScreenButton") => {
            return "button".into();
        }
        _ => {}
    }
    match role {
        "AXButton" => "button",
        "AXCheckBox" => "checkbox",
        "AXRadioButton" => "radio",
        "AXTextField" => "textfield",
        "AXTextArea" => "text-area",
        "AXStaticText" => "text",
        "AXSlider" => "slider",
        "AXIncrementor" => "stepper",
        "AXPopUpButton" => "popup",
        "AXComboBox" => "combo",
        "AXMenuButton" => "menu-button",
        "AXLink" => "link",
        "AXTabGroup" => "tabs",
        "AXTable" => "table",
        "AXOutline" => "outline",
        "AXRow" => "row",
        "AXCell" => "cell",
        "AXColumn" => "column",
        "AXScrollArea" => "scroll",
        "AXScrollBar" => "scrollbar",
        "AXImage" => "image",
        "AXWindow" => "window",
        "AXSheet" => "sheet",
        "AXDialog" => "dialog",
        "AXGroup" | "AXRadioGroup" => "group",
        "AXSplitGroup" => "split-group",
        "AXSplitter" => "splitter",
        "AXLayoutArea" => "canvas",
        "AXMenuBar" => "menu-bar",
        "AXMenuBarItem" => "menu-bar-item",
        "AXMenu" => "menu",
        "AXMenuItem" => "menu-item",
        "AXToolbar" => "toolbar",
        "AXWebArea" => "web",
        "AXList" => "list",
        "AXHeading" => "heading",
        "AXDisclosureTriangle" => "disclosure",
        "AXColorWell" => "color-well",
        "AXProgressIndicator" | "AXBusyIndicator" => "progress",
        "AXLevelIndicator" | "AXValueIndicator" => "indicator",
        "AXDateField" => "date-field",
        "AXUnknown" => "unknown",
        other => return other.trim_start_matches("AX").to_ascii_lowercase(),
    }
    .into()
}

/// A label for an unlabelled control whose subrole says what it is.
fn subrole_label(subrole: &str) -> Option<&'static str> {
    Some(match subrole {
        "AXCloseButton" => "close",
        "AXMinimizeButton" => "minimise",
        "AXZoomButton" => "zoom",
        "AXFullScreenButton" => "full screen",
        "AXIncrementArrow" => "increment",
        "AXDecrementArrow" => "decrement",
        _ => return None,
    })
}

/// Accessibility actions shown on an element's line (the role's default press is not).
fn neutral_actions(names: &[String], role: &str) -> Vec<String> {
    let pressable = matches!(
        role,
        "button"
            | "checkbox"
            | "radio"
            | "menu-item"
            | "link"
            | "tab"
            | "disclosure"
            | "menu-button"
            | "popup"
    );
    names
        .iter()
        .filter_map(|a| match a.as_str() {
            "AXPress" if !pressable => Some("press"),
            "AXIncrement" if role != "slider" && role != "stepper" => Some("increment"),
            "AXDecrement" if role != "slider" && role != "stepper" => Some("decrement"),
            // Text fields all offer confirm; it is their return key, not worth a word per line.
            "AXConfirm"
                if !matches!(
                    role,
                    "textfield" | "secure-field" | "search-field" | "combo"
                ) =>
            {
                Some("confirm")
            }
            "AXPick" => Some("pick"),
            "AXCancel" => Some("cancel"),
            _ => None,
        })
        .map(str::to_owned)
        .collect()
}

/// The accessibility action for a platform-neutral one.
pub fn ax_action(neutral: &str) -> String {
    match neutral {
        "press" => "AXPress".into(),
        "show-menu" => "AXShowMenu".into(),
        "increment" => "AXIncrement".into(),
        "decrement" => "AXDecrement".into(),
        "confirm" => "AXConfirm".into(),
        "cancel" => "AXCancel".into(),
        "raise" => "AXRaise".into(),
        "pick" => "AXPick".into(),
        "scroll-to-visible" => "AXScrollToVisible".into(),
        other if other.starts_with("AX") => other.into(),
        other => format!("AX{other}"),
    }
}

/// Roles whose actions are read (a call per element, so only where they can add something):
/// a standard control's role already says what it does.
fn reads_actions(role: &str) -> bool {
    !matches!(
        role,
        "AXStaticText"
            | "AXGroup"
            | "AXScrollArea"
            | "AXScrollBar"
            | "AXSplitGroup"
            | "AXSplitter"
            | "AXColumn"
            | "AXRow"
            | "AXTable"
            | "AXOutline"
            | "AXList"
            | "AXWindow"
            | "AXSheet"
            | "AXImage"
            | "AXHeading"
            | "AXToolbar"
            | "AXTabGroup"
            | "AXMenuBar"
            | "AXMenu"
            | "AXMenuItem"
            | "AXMenuBarItem"
            | "AXButton"
            | "AXCheckBox"
            | "AXRadioButton"
            | "AXPopUpButton"
            | "AXMenuButton"
            | "AXLink"
            | "AXDisclosureTriangle"
            | "AXSlider"
            | "AXIncrementor"
            | "AXTextField"
            | "AXTextArea"
            | "AXComboBox"
            | "AXValueIndicator"
            | "AXLevelIndicator"
            | "AXProgressIndicator"
            | "AXBusyIndicator"
    )
}

const MAX_NODES: usize = 4_000;
const MAX_DEPTH: u16 = 64;

/// Reads one element's attributes in a single call; returns the node and its children.
fn read_one(
    el: &AxEl,
    depth: u16,
    origin: Point,
    names: &CFArray<CFString>,
) -> (RawNode<AxEl>, Vec<AxEl>) {
    let mut out: *const CFArray = std::ptr::null();
    // SAFETY: `out` is a valid out pointer; a non-null result is +1 retained. The names array
    // is passed as the untyped array the call takes.
    let e = unsafe {
        el.0.copy_multiple_attribute_values(
            names.as_opaque(),
            AXCopyMultipleAttributeOptions(0),
            NonNull::from(&mut out),
        )
    };
    let mut node = RawNode::new(el.clone(), depth, "unknown");
    if e != AXError::Success {
        return (node, Vec::new());
    }
    let Some(p) = NonNull::new(out.cast_mut()) else {
        return (node, Vec::new());
    };
    // SAFETY: the values array holds CF objects, one per attribute name.
    let vals: CFRetained<CFArray<CFType>> =
        unsafe { CFRetained::cast_unchecked(CFRetained::from_raw(p)) };
    let get = |i: usize| -> Option<CFRetained<CFType>> {
        let v = vals.get(i)?;
        // A missing attribute comes back as an AXValue holding an error.
        if let Some(ax) = v.downcast_ref::<AXValue>()
            // SAFETY: reading the type tag of a live AXValue.
            && unsafe { ax.r#type() } == AXValueType::AXError
        {
            return None;
        }
        Some(v)
    };
    let role = get(0).and_then(as_string).unwrap_or_default();
    let subrole = get(1).and_then(as_string);
    let title = get(2).and_then(as_string).filter(|s| !s.is_empty());
    let desc = get(3).and_then(as_string).filter(|s| !s.is_empty());
    let placeholder = get(12).and_then(as_string).filter(|s| !s.is_empty());
    node.role = neutral_role(&role, subrole.as_deref());
    node.secure = node.role == "secure-field";
    node.label = title.or(desc).or(placeholder).or_else(|| {
        subrole
            .as_deref()
            .and_then(subrole_label)
            .map(str::to_owned)
    });
    let value = if node.secure {
        None
    } else {
        get(4).and_then(as_string)
    };
    match node.role.as_str() {
        "checkbox" | "radio" => {
            node.checked = value.as_deref().map(|v| match v {
                "1" => Check::On,
                "2" => Check::Mixed,
                _ => Check::Off,
            });
        }
        "text" if node.label.is_none() => node.label = value,
        _ => node.value = value,
    }
    let pos: Option<CGPoint> = get(5).and_then(|v| ax_value(v, AXValueType::CGPoint));
    let size: Option<CGSize> = get(6).and_then(|v| ax_value(v, AXValueType::CGSize));
    if let (Some(p), Some(s)) = (pos, size) {
        node.frame = Some(Rect::new(p.x - origin.x, p.y - origin.y, s.width, s.height));
    }
    node.enabled = get(7).and_then(as_bool).unwrap_or(true);
    node.focused = get(8).and_then(as_bool).unwrap_or(false);
    node.selected = get(9).and_then(as_bool).unwrap_or(false) && node.role != "menu-bar-item";
    node.expanded = get(10).and_then(as_bool).filter(|e| *e);
    if reads_actions(&role) {
        node.actions = neutral_actions(&el.action_names(), &node.role);
    }
    let children = match get(11).map(|v| v.downcast::<CFArray>()) {
        Some(Ok(arr)) => {
            // SAFETY: a children list holds accessibility elements.
            let arr: CFRetained<CFArray<CFType>> = unsafe { CFRetained::cast_unchecked(arr) };
            arr.iter()
                .filter_map(|x| x.downcast::<AXUIElement>().ok())
                .map(AxEl)
                .collect()
        }
        _ => Vec::new(),
    };
    (node, children)
}

/// The rows of a table, outline or list that it reports out of view. Empty when it can't say,
/// or says nothing is in view: then every row is read.
fn rows_out_of_view(list: &AxEl) -> HashSet<AxEl> {
    for (all, visible) in [
        ("AXRows", "AXVisibleRows"),
        ("AXChildren", "AXVisibleChildren"),
    ] {
        let seen: HashSet<AxEl> = list.elements(visible).into_iter().collect();
        if !seen.is_empty() {
            return list
                .elements(all)
                .into_iter()
                .filter(|r| !seen.contains(r))
                .collect();
        }
    }
    HashSet::new()
}

/// The window's elements in pre-order, frames relative to `origin` (the window's top left).
/// Unless `all` is asked for, rows a list reports out of view aren't read: they come back
/// `unread`, which keeps a long table's observation as cheap as its visible part.
pub fn tree(window: &AxEl, origin: Point, all: bool) -> Vec<RawNode<AxEl>> {
    let names = attr_names();
    let mut out = Vec::new();
    let mut stack: Vec<(AxEl, u16, bool)> = vec![(window.clone(), 0, false)];
    while let Some((el, depth, unread)) = stack.pop() {
        if out.len() >= MAX_NODES {
            break;
        }
        if unread {
            let mut node = RawNode::new(el, depth, "row");
            node.unread = true;
            out.push(node);
            continue;
        }
        let (node, children) = read_one(&el, depth, origin, &names);
        let skip = if !all && matches!(node.role.as_str(), "table" | "outline" | "list") {
            rows_out_of_view(&el)
        } else {
            HashSet::new()
        };
        out.push(node);
        if depth < MAX_DEPTH {
            for c in children.into_iter().rev() {
                let unread = skip.contains(&c);
                stack.push((c, depth + 1, unread));
            }
        }
    }
    out
}

/// One element, read fresh.
pub fn read(el: &AxEl, origin: Point) -> CuResult<RawNode<AxEl>> {
    let names = attr_names();
    let (node, _) = read_one(el, 0, origin, &names);
    if node.role == "unknown" && el.attr("AXRole").is_err() {
        return err(ErrorCode::StaleRef, "the element is gone");
    }
    Ok(node)
}
