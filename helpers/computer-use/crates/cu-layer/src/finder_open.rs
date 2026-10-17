//! Opening a file from Finder without bringing its app to the front.
//!
//! When Finder opens a document (a double-click, AXOpen on its row, ⌘O, ⌘↓,
//! File > Open or File > Open With), the app that opens it comes to the
//! front, also when Finder itself stays in the background. That app is
//! neither the call's target nor launched by it, so the restore policy
//! ([`crate::focus_policy`]) never sends it back.
//!
//! So these gestures in Finder open the files through Brigadier's
//! `launch_app` instead, which opens them behind the user's work in the app
//! Finder would use (or the one File > Open With names), and lists the new
//! window first. Folders and sidebar places stay with Finder, which shows
//! them in its own window. A gesture whose files can't be told goes to the
//! driver as before.
//!
//! - an element gesture reads the file from the element's row (`AXURL`);
//! - a key or menu gesture reads the items selected in the Finder window;
//! - a pixel double-click reads the file of the item at the point: the row
//!   (list and column views) or icon whose frame holds it, in the window's
//!   Accessibility tree, which also answers for a covered window or one on
//!   another Space. A background click never selects a Finder row, so the
//!   selection can't tell. Only when no item is there does it fall back to
//!   clicking once with the driver's own `click` and reading the selection;
//!   when nothing comes back selected the gesture is refused with a plain
//!   answer pointing at `element_token` addressing, rather than falling
//!   through to a raw double-click that would silently open nothing.
//!
//! [`install`] wraps the driver's tools; call it before
//! [`crate::guard::install`], so the guard checks the target first. The
//! nested `launch_app` call goes through the registry, so the guard checks
//! the app a file opens in too. The selecting click must not: the registry
//! runs one desktop action at a time, and the gesture being wrapped already
//! holds that turn, so a `click` through the registry would wait forever.
//! It calls the driver's `click` below this layer instead, inside the turn.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use async_trait::async_trait;
use cua_driver_core::action_record::{
    ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
    EvidenceKind, RequestedDelivery,
};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{ProtectedResourceOwnership, Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

pub const FINDER_BUNDLE_ID: &str = "com.apple.finder";

/// The tools whose gestures can open a file in Finder.
pub const TOOLS: &[&str] = &[
    "click",
    "double_click",
    "press_key",
    "hotkey",
    "invoke_menu",
];

/// A Finder gesture that opens what it points at.
#[derive(Clone, Debug, PartialEq)]
pub enum Gesture {
    /// AXOpen or a double-click on an element: the element's file.
    Element,
    /// A key or menu command on the window's selected items, in their own
    /// app or, for File > Open With, in the app it names.
    Selection { with: Option<String> },
    /// A double-click at a pixel: whatever a click there selects.
    PixelDoubleClick,
}

/// The opening gesture a call makes, if any (whatever app it targets).
pub fn gesture(tool: &str, args: &Value) -> Option<Gesture> {
    let has = |key: &str| args.get(key).is_some_and(|value| !value.is_null());
    let element = has("element_token") || has("element_index");
    let pixel = has("x") && has("y");
    match tool {
        "click" => {
            let action = args
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("press");
            let count = args.get("count").and_then(Value::as_u64).unwrap_or(1);
            if element && is_open_action(action) {
                Some(Gesture::Element)
            } else if !element && pixel && count >= 2 {
                Some(Gesture::PixelDoubleClick)
            } else {
                None
            }
        }
        "double_click" if element => Some(Gesture::Element),
        "double_click" if pixel => Some(Gesture::PixelDoubleClick),
        "press_key" => {
            let key = args.get("key").and_then(Value::as_str)?;
            let modifiers: Vec<&str> = args
                .get("modifiers")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            is_open_combo(key, &modifiers).then_some(Gesture::Selection { with: None })
        }
        "hotkey" => {
            let keys: Vec<&str> = args
                .get("keys")?
                .as_array()?
                .iter()
                .map(Value::as_str)
                .collect::<Option<_>>()?;
            let (key, modifiers) = keys.split_last()?;
            is_open_combo(key, modifiers).then_some(Gesture::Selection { with: None })
        }
        "invoke_menu" => {
            let path: Vec<&str> = args
                .get("path")?
                .as_array()?
                .iter()
                .map(|segment| segment.as_str().map(str::trim))
                .collect::<Option<_>>()?;
            menu_open(&path)
        }
        _ => None,
    }
}

fn is_open_action(action: &str) -> bool {
    let key: String = action
        .chars()
        .filter(|c| *c != '_' && *c != '-')
        .collect::<String>()
        .to_lowercase();
    key.strip_prefix("ax").unwrap_or(&key) == "open"
}

/// ⌘O or ⌘↓, Finder's shortcuts for Open.
fn is_open_combo(key: &str, modifiers: &[&str]) -> bool {
    let command = |m: &&str| matches!(m.to_ascii_lowercase().as_str(), "cmd" | "command");
    let key = key.to_ascii_lowercase().replace(['_', '-'], "");
    matches!(key.as_str(), "o" | "down" | "downarrow" | "arrowdown")
        && modifiers.len() == 1
        && modifiers.iter().all(command)
}

/// File > Open, or File > Open With > an app ("Other…" asks in Finder).
fn menu_open(path: &[&str]) -> Option<Gesture> {
    match path {
        ["File", "Open"] => Some(Gesture::Selection { with: None }),
        ["File", "Open With", app] if !app.starts_with("Other") => Some(Gesture::Selection {
            with: Some(app.trim_end_matches(" (default)").trim().to_owned()),
        }),
        _ => None,
    }
}

/// A plain left click with no modifier keys in the background, without a
/// debug image: the only pixel double-click that opens the file under it.
/// Others go to the driver's `click`, which says what it does with them.
fn plain_click(args: &Value) -> bool {
    let empty = |key: &str| {
        args.get(key)
            .is_none_or(|value| value.is_null() || value.as_array().is_some_and(Vec::is_empty))
    };
    let left = args
        .get("button")
        .and_then(Value::as_str)
        .is_none_or(|button| button.eq_ignore_ascii_case("left"));
    let background = args
        .get("delivery_mode")
        .and_then(Value::as_str)
        .is_none_or(|mode| mode.eq_ignore_ascii_case("background"));
    let press = args
        .get("action")
        .and_then(Value::as_str)
        .is_none_or(|action| action.eq_ignore_ascii_case("press"));
    // A debug image is drawn by the driver's click.
    let debug = args
        .get("debug_image_out")
        .is_some_and(|out| !out.is_null());
    left && background && press && !debug && empty("modifier") && empty("modifiers")
}

/// The pid a call names, also through its element token.
pub fn target_pid(args: &Value) -> Option<i32> {
    let pid = args.get("pid").and_then(Value::as_i64).or_else(|| {
        let token = args.get("element_token").and_then(Value::as_str)?;
        crate::tokens::owner(token).map(|(pid, _)| pid)
    })?;
    i32::try_from(pid).ok()
}

/// The app a file opens in.
#[derive(Clone, Debug, PartialEq)]
pub enum OpenIn {
    /// The app with this bundle id.
    Bundle(String),
    /// The app with this name or at this path (an `.app` opens itself).
    Name(String),
}

/// One `launch_app` call: an app and the files it opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Opening {
    pub app: OpenIn,
    pub files: Vec<PathBuf>,
}

/// What Finder knows: which pid it is, and the files of its rows.
pub trait Finder: Send + Sync {
    fn is_finder(&self, pid: i32) -> bool;
    /// The files the element a call names stands for (its row's `AXURL`).
    fn element_files(&self, pid: i32, args: &Value) -> Vec<PathBuf>;
    /// The files selected in `window_id` (else Finder's focused window).
    fn selected_files(&self, pid: i32, window_id: Option<u32>) -> Vec<PathBuf>;
    /// The file of the item at the pixel `x`, `y` of `args` in `window_id`
    /// (see [`file_at`]).
    fn files_at(&self, pid: i32, window_id: u32, args: &Value) -> Vec<PathBuf>;
    /// A folder Finder shows in a window (not a package such as an app).
    fn is_folder(&self, path: &Path) -> bool;
    /// The bundle id of the app Finder would open `path` in.
    fn default_app(&self, path: &Path) -> Option<String>;
}

/// One element of a window's Accessibility tree, for [`file_at`].
pub trait Item: Sized {
    fn role(&self) -> Option<String>;
    /// `[x, y, width, height]` on screen.
    fn frame(&self) -> Option<[f64; 4]>;
    /// The local file the element itself names (its `AXURL`).
    fn file(&self) -> Option<PathBuf>;
    /// The elements in it; for a list, the rows it shows when it says which.
    fn children(&self) -> Vec<Self>;
}

/// Elements looked at for one point; far more than the rows a window shows.
const MAX_ITEMS: usize = 3000;
/// How deep under a row the file's name field sits (row → cell → field).
const ROW_DEPTH: usize = 3;

/// The file of the item at screen `point` in `window`: the row that holds
/// it (list and column views) or an element that names a file itself (an
/// icon) whose frame holds it. An element whose frame doesn't hold the point
/// is passed over with all it contains, so a row scrolled out of its list
/// never counts; one without a frame is looked into but never taken itself.
pub fn file_at<I: Item>(window: &I, point: (f64, f64)) -> Option<PathBuf> {
    let mut budget = MAX_ITEMS;
    item_file_at(window, point, &mut budget)
}

fn item_file_at<I: Item>(element: &I, (x, y): (f64, f64), budget: &mut usize) -> Option<PathBuf> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    let holds = element.frame().map(|[left, top, width, height]| {
        x >= left && y >= top && x < left + width && y < top + height
    });
    if holds == Some(false) {
        return None;
    }
    if holds == Some(true) {
        if element.role().as_deref() == Some("AXRow") {
            return file_within(element, ROW_DEPTH);
        }
        if let Some(file) = element.file() {
            return Some(file);
        }
    }
    element
        .children()
        .iter()
        .find_map(|child| item_file_at(child, (x, y), budget))
}

/// The file `element` or an element up to `depth` below it names.
fn file_within<I: Item>(element: &I, depth: usize) -> Option<PathBuf> {
    element.file().or_else(|| {
        (depth > 0)
            .then(|| {
                element
                    .children()
                    .iter()
                    .find_map(|child| file_within(child, depth - 1))
            })
            .flatten()
    })
}

/// Calls the registry the tools are in (the guard included).
#[async_trait]
pub trait Calls: Send + Sync {
    async fn call(&self, tool: &str, args: Value) -> ToolResult;
}

/// The `launch_app` calls that open `files` (folders already left out), in
/// order, one per app. `None` when a file's app is unknown.
pub fn plan(files: &[PathBuf], with: Option<&str>, finder: &dyn Finder) -> Option<Vec<Opening>> {
    let mut openings: Vec<Opening> = Vec::new();
    for file in files {
        let app = match with {
            Some(name) => OpenIn::Name(name.to_owned()),
            None if file.extension().is_some_and(|ext| ext == "app") => {
                OpenIn::Name(file.to_string_lossy().into_owned())
            }
            None => OpenIn::Bundle(finder.default_app(file)?),
        };
        if let OpenIn::Name(name) = &app {
            if name.ends_with(".app") && with.is_none() {
                // An app opens itself, with no document.
                openings.push(Opening { app, files: vec![] });
                continue;
            }
        }
        match openings.iter_mut().find(|opening| opening.app == app) {
            Some(opening) => opening.files.push(file.clone()),
            None => openings.push(Opening {
                app,
                files: vec![file.clone()],
            }),
        }
    }
    Some(openings)
}

/// The `launch_app` arguments for one opening.
pub fn launch_args(opening: &Opening, session: Option<&Value>) -> Value {
    let mut args = match &opening.app {
        OpenIn::Bundle(bundle_id) => json!({ "bundle_id": bundle_id }),
        OpenIn::Name(name) => json!({ "name": name }),
    };
    if !opening.files.is_empty() {
        args["urls"] = json!(opening
            .files
            .iter()
            .map(|file| file.to_string_lossy())
            .collect::<Vec<_>>());
    }
    if let Some(session) = session {
        args["session"] = session.clone();
    }
    args
}

/// Puts Brigadier's Finder open in front of the driver's tools.
#[cfg(target_os = "macos")]
pub fn install(registry: &mut ToolRegistry) {
    install_with(registry, Arc::new(System), Arc::new(RegistryCalls));
}

/// [`install`] with the given Finder and registry calls.
pub fn install_with(registry: &mut ToolRegistry, finder: Arc<dyn Finder>, calls: Arc<dyn Calls>) {
    // The driver's `click` under this layer, for the pixel double-click's
    // selecting click (`TOOLS` wraps `click` first).
    let click: Arc<OnceLock<Arc<dyn Tool>>> = Arc::default();
    for tool in TOOLS {
        if registry.tool_names().all(|name| name != *tool) {
            continue;
        }
        let finder = finder.clone();
        let calls = calls.clone();
        let click = click.clone();
        registry.wrap(tool, move |inner| {
            let inner: Arc<dyn Tool> = Arc::from(inner);
            if inner.def().name == "click" {
                let _ = click.set(inner.clone());
            }
            Box::new(FinderOpen {
                inner,
                finder,
                calls,
                click,
            }) as Box<dyn Tool>
        });
    }
}

struct FinderOpen {
    inner: Arc<dyn Tool>,
    finder: Arc<dyn Finder>,
    calls: Arc<dyn Calls>,
    click: Arc<OnceLock<Arc<dyn Tool>>>,
}

#[async_trait]
impl Tool for FinderOpen {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    fn has_independent_input_lane(&self, args: &Value) -> bool {
        self.inner.has_independent_input_lane(args)
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        self.inner
            .protected_resource_ownership(adapter_id, args)
            .await
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        self.inner.protected_resource_scope(adapter_id, args).await
    }

    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), String> {
        self.inner
            .validate_protected_resource_scope(adapter_id, args, approved_scope)
            .await
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let tool = self.inner.def().name.clone();
        let Some(gesture) = gesture(&tool, &args) else {
            return self.inner.invoke(args).await;
        };
        let Some(pid) = target_pid(&args).filter(|pid| self.finder.is_finder(*pid)) else {
            return self.inner.invoke(args).await;
        };
        let window_id = args
            .get("window_id")
            .and_then(Value::as_u64)
            .and_then(|id| u32::try_from(id).ok());
        let (items, with) = match gesture {
            Gesture::Element => (self.finder.element_files(pid, &args), None),
            Gesture::Selection { with } => (self.finder.selected_files(pid, window_id), with),
            Gesture::PixelDoubleClick => {
                let at_point = window_id
                    .filter(|_| plain_click(&args))
                    .map(|window_id| self.finder.files_at(pid, window_id, &args))
                    .unwrap_or_default();
                if !at_point.is_empty() {
                    return self.open_items(at_point, None, args).await;
                }
                let Some(click) = self.click.get() else {
                    return self.inner.invoke(args).await;
                };
                // The first click of the double-click selects the item.
                let mut first = args.clone();
                if let Some(object) = first.as_object_mut() {
                    object.insert("count".to_owned(), json!(1));
                }
                let selected = click.invoke(first).await;
                if selected.is_error == Some(true) {
                    return selected;
                }
                let items = self.finder.selected_files(pid, window_id);
                if items.is_empty() {
                    // The selecting click landed without selecting a file row
                    // we can read back (a miss, or a non-file target). Say so
                    // plainly rather than falling through to a raw
                    // double-click: that posts a background CGEvent Finder
                    // may not act on at all, silently opening nothing while
                    // still answering ok.
                    return pixel_double_click_unresolved();
                }
                (items, None)
            }
        };
        self.open_items(items, with, args).await
    }
}

impl FinderOpen {
    /// Opens `items` (folders stay with Finder) behind the user's work, or
    /// hands the call to the driver when there is nothing it can open so.
    async fn open_items(
        &self,
        items: Vec<PathBuf>,
        with: Option<String>,
        args: Value,
    ) -> ToolResult {
        let files: Vec<PathBuf> = items
            .into_iter()
            .filter(|item| !self.finder.is_folder(item))
            .collect();
        if files.is_empty() {
            // Folders and places: Finder shows them itself.
            return self.inner.invoke(args).await;
        }
        let Some(openings) = plan(&files, with.as_deref(), self.finder.as_ref()) else {
            return self.inner.invoke(args).await;
        };
        self.open(openings, args.get("session")).await
    }

    async fn open(&self, openings: Vec<Opening>, session: Option<&Value>) -> ToolResult {
        let mut texts = Vec::new();
        let mut first: Option<Value> = None;
        let mut opened = Vec::new();
        for opening in &openings {
            let result = self
                .calls
                .call("launch_app", launch_args(opening, session))
                .await;
            if result.is_error == Some(true) {
                return result;
            }
            let names: Vec<String> = opening
                .files
                .iter()
                .map(|file| {
                    file.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| file.to_string_lossy().into_owned())
                })
                .collect();
            let structured = result.structured_content.clone().unwrap_or(json!({}));
            let app = structured
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("its app")
                .to_owned();
            let what = if names.is_empty() {
                format!("Opened {app}")
            } else {
                format!("Opened {} in {app}", names.join(", "))
            };
            texts.push(format!(
                "{what} behind the user's work, as Finder's Open would but without bringing \
                 {app} to the front.\n{}",
                text_of(&result)
            ));
            opened.extend(
                opening
                    .files
                    .iter()
                    .map(|file| file.to_string_lossy().into_owned()),
            );
            first.get_or_insert(structured);
        }
        let mut structured = first.unwrap_or(json!({}));
        structured["opened_from_finder"] = json!(opened);
        // Action tools answer with the closed action record; the opened
        // app's pid and windows are in the text.
        let shown = structured
            .get("windows")
            .and_then(Value::as_array)
            .is_some_and(|windows| !windows.is_empty());
        let result = ToolResult::text(texts.join("\n\n")).with_structured(structured);
        let record = if shown {
            ActionExecutionRecord::builder(
                ActionEffect::Confirmed,
                ActionTransport::MacosAxAction,
                RequestedDelivery::Background,
            )
            .evidence(ActionEvidence {
                kind: EvidenceKind::WindowChange,
                detail: "the opened file's window is listed".into(),
            })
        } else {
            ActionExecutionRecord::builder(
                ActionEffect::Unverifiable,
                ActionTransport::MacosAxAction,
                RequestedDelivery::Background,
            )
        };
        match record.actual_delivery(ActualDelivery::Background).build() {
            Ok(record) => result.with_action_record(record),
            Err(_) => result,
        }
    }
}

fn text_of(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            cua_driver_core::protocol::Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A pixel double-click in Finder whose selecting click didn't land on a
/// file row we can read back.
fn pixel_double_click_unresolved() -> ToolResult {
    let message = "double_click was not done: the point didn't land on a file row Brigadier \
         could read back. Call get_window_state for the window, then double_click (or click \
         action:\"open\") the row by its element_token instead — that opens the file behind \
         the user's work."
        .to_owned();
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "code": "pixel_open_unresolved",
        "refusal": { "code": "pixel_open_unresolved", "message": message },
    }))
}

/// The registry [`registry::build`](crate::registry) made last, for the
/// nested calls.
static REGISTRY: Mutex<Option<Weak<ToolRegistry>>> = Mutex::new(None);

/// Remember `registry` for [`RegistryCalls`]. Call it once the registry is
/// in its `Arc`.
pub fn set_registry(registry: &Arc<ToolRegistry>) {
    *REGISTRY.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::downgrade(registry));
}

/// Calls through the registry [`set_registry`] remembered.
pub struct RegistryCalls;

#[async_trait]
impl Calls for RegistryCalls {
    async fn call(&self, tool: &str, args: Value) -> ToolResult {
        let registry = REGISTRY
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .and_then(Weak::upgrade);
        match registry {
            Some(registry) => registry.invoke(tool, args).await,
            None => ToolResult::error(format!("{tool} is not available right now.")),
        }
    }
}

/// The real Finder, through Accessibility and LaunchServices.
#[cfg(target_os = "macos")]
pub struct System;

#[cfg(target_os = "macos")]
impl Finder for System {
    fn is_finder(&self, pid: i32) -> bool {
        crate::launch::apps::running_app(pid)
            .and_then(|app| app.bundle_id)
            .as_deref()
            == Some(FINDER_BUNDLE_ID)
    }

    fn element_files(&self, pid: i32, args: &Value) -> Vec<PathBuf> {
        system::element_files(pid, args)
    }

    fn selected_files(&self, pid: i32, window_id: Option<u32>) -> Vec<PathBuf> {
        unsafe { system::selected_files(pid, window_id) }
    }

    fn files_at(&self, pid: i32, window_id: u32, args: &Value) -> Vec<PathBuf> {
        system::files_at(pid, window_id, args)
    }

    fn is_folder(&self, path: &Path) -> bool {
        path.is_dir() && !system::is_package(path)
    }

    fn default_app(&self, path: &Path) -> Option<String> {
        system::default_app(path)
    }
}

#[cfg(target_os = "macos")]
mod system {
    use std::path::{Path, PathBuf};

    use core_foundation::array::CFArray;
    use core_foundation::base::{CFGetTypeID, CFRelease, CFRetain, CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use objc2::rc::Retained;
    use objc2::{msg_send_id, ClassType};
    use objc2_app_kit::NSWorkspace;
    use objc2_foundation::{NSBundle, NSString, NSURL};
    use platform_macos::ax::bindings::{
        ax_get_window_id, copy_ax_windows_including, copy_children, copy_element_attr,
        copy_string_attr, copy_url_attr, element_screen_rect, AXUIElementCopyAttributeValue,
        AXUIElementCreateApplication, AXUIElementGetTypeID, AXUIElementRef,
    };
    use serde_json::Value;

    use super::ROW_DEPTH;

    /// Elements looked at for the selection; a Finder window has far fewer
    /// rows' worth of containers.
    const MAX_ELEMENTS: usize = 3000;

    pub fn element_files(pid: i32, args: &Value) -> Vec<PathBuf> {
        use cua_driver_core::element_token::ResolvedElement;
        use platform_macos::ax::snapshot::AxSnapshot;

        let Some(store) = cua_driver_core::snapshot_store::current_runtime_store::<AxSnapshot>()
        else {
            return vec![];
        };
        let Ok(ResolvedElement::Element { element, .. }) = store.resolve(pid, args) else {
            return vec![];
        };
        unsafe { row_files(element.as_ptr() as AXUIElementRef) }
    }

    /// The file of the row `element` is, or is inside.
    unsafe fn row_files(element: AXUIElementRef) -> Vec<PathBuf> {
        if let Some(file) = file_within(element, ROW_DEPTH) {
            return vec![file];
        }
        // An image or label inside the row: its row names the file.
        let Some(parent) = copy_element_attr(element, "AXParent") else {
            return vec![];
        };
        let file = file_within(parent, ROW_DEPTH);
        CFRelease(parent as CFTypeRef);
        file.into_iter().collect()
    }

    /// `window_id` of `pid` (else its focused window), also on another
    /// Space, retained for the caller to release.
    unsafe fn copy_window(pid: i32, window_id: Option<u32>) -> Option<AXUIElementRef> {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let window = match window_id {
            Some(window_id) => {
                let mut found = None;
                for window in copy_ax_windows_including(app, pid, window_id) {
                    if found.is_none() && ax_get_window_id(window) == Some(window_id) {
                        found = Some(window);
                    } else {
                        CFRelease(window as CFTypeRef);
                    }
                }
                found
            }
            None => copy_element_attr(app, "AXFocusedWindow")
                .or_else(|| copy_element_attr(app, "AXMainWindow")),
        };
        CFRelease(app as CFTypeRef);
        window
    }

    /// The file of the item at the call's pixel in `window_id`: the pixel
    /// mapped to the screen as the driver's `click` maps it (the session's
    /// screenshot or zoom of the window, then the window's frame and capture
    /// scale), then [`super::file_at`] in the window's tree. Empty when any
    /// step can't be proven.
    pub fn files_at(pid: i32, window_id: u32, args: &Value) -> Vec<PathBuf> {
        let Some(point) = screen_point(pid, window_id, args) else {
            return vec![];
        };
        let Some(window) = (unsafe { copy_window(pid, Some(window_id)) }) else {
            return vec![];
        };
        super::file_at(&Element(window), point)
            .into_iter()
            .collect()
    }

    fn screen_point(pid: i32, window_id: u32, args: &Value) -> Option<(f64, f64)> {
        use cua_driver_core::snapshot_store::current_runtime_store;
        use platform_macos::ax::snapshot::AxSnapshot;

        let x = args.get("x").and_then(Value::as_f64)?;
        let y = args.get("y").and_then(Value::as_f64)?;
        // A capture-bound point is admitted by the driver's own capture check.
        if args.get("capture_id").is_some_and(|id| !id.is_null()) {
            return None;
        }
        let store = current_runtime_store::<AxSnapshot>()?;
        let window = Some(u64::from(window_id));
        let (x, y) = if args.get("from_zoom").and_then(Value::as_bool) == Some(true) {
            let session = args.get("_session_id").and_then(Value::as_str);
            store.zoom(pid, window, session).ok()?.zoom_to_window(x, y)
        } else {
            let ratio = store.screenshot_scale(pid, window, args).ok()?;
            (x * ratio, y * ratio)
        };
        let bounds = platform_macos::windows::window_bounds_by_id(window_id)
            .filter(|bounds| bounds.width > 0.0 && bounds.height > 0.0)?;
        let png = platform_macos::capture::screenshot_window_bytes(window_id).ok()?;
        let (width, height) = platform_macos::capture::png_dimensions(&png).ok()?;
        let scale = capture_scale(
            f64::from(width) / bounds.width,
            f64::from(height) / bounds.height,
        )?;
        let (x, y) = (x / scale, y / scale);
        (x >= 0.0 && y >= 0.0 && x <= bounds.width && y <= bounds.height)
            .then(|| (bounds.x + x, bounds.y + y))
    }

    /// A window capture is 1× or 2× its frame; anything else is another
    /// surface's.
    fn capture_scale(scale_x: f64, scale_y: f64) -> Option<f64> {
        let nearest = if (scale_x - 1.0).abs() <= (scale_x - 2.0).abs() {
            1.0
        } else {
            2.0
        };
        ((scale_x - scale_y).abs() <= 0.03
            && (scale_x - nearest).abs() <= 0.03
            && (scale_y - nearest).abs() <= 0.03)
            .then_some(nearest)
    }

    /// An element, released when dropped.
    struct Element(AXUIElementRef);

    impl Drop for Element {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0 as CFTypeRef) };
        }
    }

    impl super::Item for Element {
        fn role(&self) -> Option<String> {
            unsafe { copy_string_attr(self.0, "AXRole") }
        }

        fn frame(&self) -> Option<[f64; 4]> {
            unsafe { element_screen_rect(self.0) }
        }

        fn file(&self) -> Option<PathBuf> {
            unsafe { copy_url_attr(self.0) }
                .filter(|url| url.starts_with("file://"))
                .and_then(|url| file_path(&url))
        }

        fn children(&self) -> Vec<Self> {
            unsafe {
                let mut children = copy_elements_attr(self.0, "AXVisibleRows");
                if children.is_empty() {
                    children = copy_children(self.0);
                }
                children.into_iter().map(Element).collect()
            }
        }
    }

    pub unsafe fn selected_files(pid: i32, window_id: Option<u32>) -> Vec<PathBuf> {
        let Some(window) = copy_window(pid, window_id) else {
            return vec![];
        };
        let mut files = Vec::new();
        let mut pending = vec![window];
        let mut seen = 0;
        while let Some(element) = pending.pop() {
            seen += 1;
            if seen <= MAX_ELEMENTS {
                let mut selected = copy_elements_attr(element, "AXSelectedRows");
                if selected.is_empty() {
                    selected = copy_elements_attr(element, "AXSelectedChildren");
                }
                if selected.is_empty() {
                    pending.extend(copy_children(element));
                }
                for item in selected {
                    if let Some(file) = file_within(item, ROW_DEPTH) {
                        if !files.contains(&file) {
                            files.push(file);
                        }
                    }
                    CFRelease(item as CFTypeRef);
                }
            }
            CFRelease(element as CFTypeRef);
        }
        files
    }

    /// The local file `element` or an element up to `depth` below it names.
    unsafe fn file_within(element: AXUIElementRef, depth: usize) -> Option<PathBuf> {
        if let Some(file) = copy_url_attr(element)
            .filter(|url| url.starts_with("file://"))
            .and_then(|url| file_path(&url))
        {
            return Some(file);
        }
        if depth == 0 {
            return None;
        }
        let mut found = None;
        for child in copy_children(element) {
            if found.is_none() {
                found = file_within(child, depth - 1);
            }
            CFRelease(child as CFTypeRef);
        }
        found
    }

    /// An attribute holding elements (`AXSelectedRows`…), retained for the
    /// caller to release.
    unsafe fn copy_elements_attr(element: AXUIElementRef, name: &str) -> Vec<AXUIElementRef> {
        let attr = CFString::new(name);
        let mut value: CFTypeRef = std::ptr::null();
        let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
        if err != 0 || value.is_null() {
            return vec![];
        }
        if CFGetTypeID(value) != CFArray::<CFTypeRef>::type_id() {
            CFRelease(value);
            return vec![];
        }
        let array = CFArray::<CFTypeRef>::wrap_under_create_rule(value as _);
        let element_type = AXUIElementGetTypeID();
        array
            .iter()
            .filter(|item| CFGetTypeID(**item) == element_type)
            .map(|item| {
                CFRetain(*item);
                *item as AXUIElementRef
            })
            .collect()
    }

    /// The path a file URL names. Finder's rows give file reference URLs
    /// (`file:///.file/id=…`), which name the file by its id, not its path.
    pub(super) fn file_path(url: &str) -> Option<PathBuf> {
        let path = unsafe {
            NSURL::URLWithString(&NSString::from_str(url))?
                .filePathURL()?
                .path()?
        }
        .to_string();
        path.starts_with('/').then(|| PathBuf::from(path))
    }

    pub fn is_package(path: &Path) -> bool {
        let path = NSString::from_str(&path.to_string_lossy());
        unsafe { NSWorkspace::sharedWorkspace().isFilePackageAtPath(&path) }
    }

    pub fn default_app(path: &Path) -> Option<String> {
        unsafe {
            // `fileURLWithPath:` as an optional: the generated binding panics
            // on nil.
            let path = NSString::from_str(&path.to_string_lossy());
            let url: Option<Retained<NSURL>> =
                msg_send_id![NSURL::class(), fileURLWithPath: &*path];
            let url = url?;
            let app = NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&url)?;
            let bundle = NSBundle::bundleWithURL(&app)?;
            Some(bundle.bundleIdentifier()?.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_gestures_are_told_apart_from_other_calls() {
        let open = |tool: &str, args: Value| gesture(tool, &args);
        assert_eq!(
            open("click", json!({"element_token": "s1:3", "action": "open"})),
            Some(Gesture::Element)
        );
        assert_eq!(
            open(
                "click",
                json!({"element_token": "s1:3", "action": "AXOpen"})
            ),
            Some(Gesture::Element)
        );
        assert_eq!(open("click", json!({"element_token": "s1:3"})), None);
        assert_eq!(open("click", json!({"pid": 1, "x": 4, "y": 5})), None);
        assert_eq!(
            open("click", json!({"pid": 1, "x": 4, "y": 5, "count": 2})),
            Some(Gesture::PixelDoubleClick)
        );
        assert_eq!(
            open("double_click", json!({"element_token": "s1:3"})),
            Some(Gesture::Element)
        );
        assert_eq!(
            open("double_click", json!({"pid": 1, "x": 4, "y": 5})),
            Some(Gesture::PixelDoubleClick)
        );
        let selection = Some(Gesture::Selection { with: None });
        assert_eq!(
            open(
                "press_key",
                json!({"pid": 1, "key": "o", "modifiers": ["cmd"]})
            ),
            selection
        );
        assert_eq!(
            open(
                "press_key",
                json!({"pid": 1, "key": "down", "modifiers": ["command"]})
            ),
            selection
        );
        assert_eq!(open("press_key", json!({"pid": 1, "key": "o"})), None);
        assert_eq!(
            open(
                "press_key",
                json!({"pid": 1, "key": "o", "modifiers": ["cmd", "shift"]})
            ),
            None
        );
        assert_eq!(open("press_key", json!({"pid": 1, "key": "return"})), None);
        assert_eq!(
            open("hotkey", json!({"pid": 1, "keys": ["cmd", "o"]})),
            selection
        );
        assert_eq!(
            open("hotkey", json!({"pid": 1, "keys": ["cmd", "a"]})),
            None
        );
        assert_eq!(
            open("invoke_menu", json!({"pid": 1, "path": ["File", "Open"]})),
            selection
        );
        assert_eq!(
            open(
                "invoke_menu",
                json!({"pid": 1, "path": ["File", "Open With", "TextEdit (default)"]})
            ),
            Some(Gesture::Selection {
                with: Some("TextEdit".to_owned())
            })
        );
        assert_eq!(
            open(
                "invoke_menu",
                json!({"pid": 1, "path": ["File", "Open With", "Other…"]})
            ),
            None
        );
        assert_eq!(
            open("invoke_menu", json!({"pid": 1, "path": ["File", "Rename"]})),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn finder_file_reference_urls_resolve_to_paths() {
        use objc2_foundation::{NSString, NSURL};

        let dir = std::env::temp_dir().join(format!("cu-layer-finder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a b.txt");
        std::fs::write(&file, "x").unwrap();
        let path = file.canonicalize().unwrap();
        let reference = unsafe {
            NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
                .fileReferenceURL()
                .unwrap()
                .absoluteString()
                .unwrap()
        }
        .to_string();
        assert!(reference.starts_with("file:///.file/id="), "{reference}");
        assert_eq!(system::file_path(&reference), Some(path.clone()));
        let plain = format!("file://{}", path.to_string_lossy().replace(' ', "%20"));
        assert_eq!(system::file_path(&plain), Some(path));
        assert_eq!(system::default_app(Path::new("")), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    struct Folders;

    impl Finder for Folders {
        fn is_finder(&self, _pid: i32) -> bool {
            true
        }
        fn element_files(&self, _pid: i32, _args: &Value) -> Vec<PathBuf> {
            vec![]
        }
        fn selected_files(&self, _pid: i32, _window_id: Option<u32>) -> Vec<PathBuf> {
            vec![]
        }
        fn files_at(&self, _pid: i32, _window_id: u32, _args: &Value) -> Vec<PathBuf> {
            vec![]
        }
        fn is_folder(&self, _path: &Path) -> bool {
            false
        }
        fn default_app(&self, path: &Path) -> Option<String> {
            match path.extension()?.to_str()? {
                "txt" | "rtf" => Some("com.apple.TextEdit".to_owned()),
                "png" => Some("com.apple.Preview".to_owned()),
                _ => None,
            }
        }
    }

    /// A fake Accessibility element.
    struct Fake {
        role: &'static str,
        frame: Option<[f64; 4]>,
        file: Option<&'static str>,
        children: Vec<Fake>,
    }

    impl Item for &Fake {
        fn role(&self) -> Option<String> {
            Some(self.role.to_owned())
        }
        fn frame(&self) -> Option<[f64; 4]> {
            self.frame
        }
        fn file(&self) -> Option<PathBuf> {
            self.file.map(PathBuf::from)
        }
        fn children(&self) -> Vec<Self> {
            self.children.iter().collect()
        }
    }

    fn el(role: &'static str, frame: [f64; 4], children: Vec<Fake>) -> Fake {
        Fake {
            role,
            frame: Some(frame),
            file: None,
            children,
        }
    }

    fn named(role: &'static str, frame: [f64; 4], file: &'static str) -> Fake {
        Fake {
            file: Some(file),
            ..el(role, frame, vec![])
        }
    }

    /// A list row as Finder shows it: row → cells → icon and name field (the
    /// field names the file), then the date cell.
    fn row(y: f64, file: &'static str) -> Fake {
        el(
            "AXRow",
            [563.0, y, 761.0, 20.0],
            vec![
                el(
                    "AXCell",
                    [589.0, y, 326.0, 20.0],
                    vec![
                        el("AXImage", [589.0, y + 2.0, 16.0, 16.0], vec![]),
                        named("AXTextField", [609.0, y + 1.0, 59.0, 18.0], file),
                    ],
                ),
                el("AXCell", [915.0, y, 181.0, 20.0], vec![]),
            ],
        )
    }

    #[test]
    fn the_item_at_a_point_is_the_row_or_icon_holding_it() {
        let list = el(
            "AXWindow",
            [404.0, 180.0, 920.0, 436.0],
            vec![
                // The sidebar: rows without files.
                el(
                    "AXScrollArea",
                    [404.0, 232.0, 159.0, 384.0],
                    vec![el("AXRow", [404.0, 251.0, 159.0, 32.0], vec![])],
                ),
                el(
                    "AXScrollArea",
                    [563.0, 259.0, 761.0, 358.0],
                    vec![Fake {
                        role: "AXOutline",
                        frame: None,
                        file: None,
                        children: vec![
                            // Scrolled up under the column header.
                            row(239.0, "/f/hidden.txt"),
                            row(265.0, "/f/alpha.txt"),
                            row(285.0, "/f/beta.txt"),
                        ],
                    }],
                ),
            ],
        );
        let at = |x, y| file_at(&&list, (x, y));
        // On the name, or anywhere else on the row.
        assert_eq!(at(630.0, 275.0), Some(PathBuf::from("/f/alpha.txt")));
        assert_eq!(at(1000.0, 295.0), Some(PathBuf::from("/f/beta.txt")));
        // Under the header, over a row scrolled out of the list.
        assert_eq!(at(630.0, 250.0), None);
        // Below the last row, in the sidebar, outside the window.
        assert_eq!(at(900.0, 500.0), None);
        assert_eq!(at(480.0, 260.0), None);
        assert_eq!(at(100.0, 100.0), None);
        // A row without a frame is never taken for a point on it.
        let unframed = el(
            "AXWindow",
            [404.0, 180.0, 920.0, 436.0],
            vec![Fake {
                frame: None,
                ..row(265.0, "/f/alpha.txt")
            }],
        );
        assert_eq!(file_at(&&unframed, (1000.0, 275.0)), None);
        // Its name field, framed, still names its file.
        assert_eq!(
            file_at(&&unframed, (630.0, 275.0)),
            Some(PathBuf::from("/f/alpha.txt"))
        );

        let icons = el(
            "AXWindow",
            [404.0, 180.0, 920.0, 436.0],
            vec![el(
                "AXList",
                [573.0, 242.0, 336.0, 112.0],
                vec![
                    el(
                        "AXGroup",
                        [597.0, 246.0, 64.0, 64.0],
                        vec![named("AXImage", [597.0, 246.0, 64.0, 64.0], "/f/alpha.txt")],
                    ),
                    el(
                        "AXGroup",
                        [709.0, 246.0, 64.0, 64.0],
                        vec![named("AXImage", [709.0, 246.0, 64.0, 64.0], "/f/beta.txt")],
                    ),
                ],
            )],
        );
        assert_eq!(
            file_at(&&icons, (740.0, 280.0)),
            Some(PathBuf::from("/f/beta.txt"))
        );
        assert_eq!(file_at(&&icons, (680.0, 280.0)), None);
    }

    #[test]
    fn files_open_in_their_own_apps_one_launch_per_app() {
        let files: Vec<PathBuf> = ["/f/a.txt", "/f/b.png", "/f/c.rtf", "/f/Tool.app"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let openings = plan(&files, None, &Folders).unwrap();
        assert_eq!(
            openings,
            vec![
                Opening {
                    app: OpenIn::Bundle("com.apple.TextEdit".into()),
                    files: vec!["/f/a.txt".into(), "/f/c.rtf".into()],
                },
                Opening {
                    app: OpenIn::Bundle("com.apple.Preview".into()),
                    files: vec!["/f/b.png".into()],
                },
                Opening {
                    app: OpenIn::Name("/f/Tool.app".into()),
                    files: vec![],
                },
            ]
        );
        assert_eq!(
            launch_args(&openings[0], Some(&json!("s"))),
            json!({"bundle_id": "com.apple.TextEdit", "urls": ["/f/a.txt", "/f/c.rtf"], "session": "s"})
        );
        assert_eq!(
            launch_args(&openings[2], None),
            json!({"name": "/f/Tool.app"})
        );
        // A file no app opens: left to Finder, which says so itself.
        assert_eq!(plan(&[PathBuf::from("/f/x.zzz")], None, &Folders), None);
        // Open With names the app for every file.
        assert_eq!(
            plan(&files[..2], Some("TextEdit"), &Folders).unwrap(),
            vec![Opening {
                app: OpenIn::Name("TextEdit".into()),
                files: files[..2].to_vec(),
            }]
        );
    }
}
