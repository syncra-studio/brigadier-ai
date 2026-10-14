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
//! - a pixel double-click reads the row under the point.
//!
//! [`install`] wraps the driver's tools; call it before
//! [`crate::guard::install`], so the guard checks the target first. The
//! nested `launch_app` call goes through the registry, so the guard checks
//! the app a file opens in too. It must stay the only nested call: the
//! registry runs one desktop action at a time, and the gesture being
//! wrapped already holds that turn, so a nested `click` would wait forever.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

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
    /// A double-click at a pixel: the row under it.
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
    /// The files of the row at window-local screenshot pixel `x`, `y` of
    /// `window_id`.
    fn point_files(&self, pid: i32, window_id: Option<u32>, x: f64, y: f64) -> Vec<PathBuf>;
    /// A folder Finder shows in a window (not a package such as an app).
    fn is_folder(&self, path: &Path) -> bool;
    /// The bundle id of the app Finder would open `path` in.
    fn default_app(&self, path: &Path) -> Option<String>;
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
    for tool in TOOLS {
        if registry.tool_names().all(|name| name != *tool) {
            continue;
        }
        let finder = finder.clone();
        let calls = calls.clone();
        registry.wrap(tool, move |inner| {
            Box::new(FinderOpen {
                inner,
                finder,
                calls,
            }) as Box<dyn Tool>
        });
    }
}

struct FinderOpen {
    inner: Box<dyn Tool>,
    finder: Arc<dyn Finder>,
    calls: Arc<dyn Calls>,
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
                let at = |key: &str| args.get(key).and_then(Value::as_f64).unwrap_or(-1.0);
                (
                    self.finder.point_files(pid, window_id, at("x"), at("y")),
                    None,
                )
            }
        };
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
}

impl FinderOpen {
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

    fn point_files(&self, pid: i32, window_id: Option<u32>, x: f64, y: f64) -> Vec<PathBuf> {
        match window_id {
            Some(window_id) => unsafe { system::point_files(pid, window_id, x, y) },
            None => vec![],
        }
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
        copy_url_attr, AXUIElementCopyAttributeValue, AXUIElementCopyElementAtPosition,
        AXUIElementCreateApplication, AXUIElementGetTypeID, AXUIElementRef,
    };
    use serde_json::Value;

    /// Elements looked at for the selection; a Finder window has far fewer
    /// rows' worth of containers.
    const MAX_ELEMENTS: usize = 3000;
    /// How deep under a row the file's name field sits (row → cell → field).
    const ROW_DEPTH: usize = 3;

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

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> i32;
    }

    /// The row of `pid`'s app at window-local screenshot pixel `x`, `y` of
    /// `window_id`, by an Accessibility hit test: no event is sent. A point
    /// that `pid` doesn't own there, such as one covered by another app's
    /// window, has no files.
    pub unsafe fn point_files(pid: i32, window_id: u32, x: f64, y: f64) -> Vec<PathBuf> {
        if x < 0.0 || y < 0.0 {
            return vec![];
        }
        let Some(bounds) = platform_macos::windows::window_bounds_by_id(window_id)
            .filter(|bounds| bounds.width > 0.0 && bounds.height > 0.0)
        else {
            return vec![];
        };
        // Screenshot pixels per point, from the window's own capture (1× or 2×).
        let Some(scale) = platform_macos::capture::screenshot_window_bytes(window_id)
            .ok()
            .and_then(|png| platform_macos::capture::png_dimensions(&png).ok())
            .map(|(width, _)| f64::from(width) / bounds.width)
            .filter(|scale| (scale - 1.0).abs() < 0.05 || (scale - 2.0).abs() < 0.05)
        else {
            return vec![];
        };
        let scale = scale.round();
        let (screen_x, screen_y) = (bounds.x + x / scale, bounds.y + y / scale);
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return vec![];
        }
        let mut hit: AXUIElementRef = std::ptr::null_mut();
        let err = AXUIElementCopyElementAtPosition(app, screen_x as f32, screen_y as f32, &mut hit);
        CFRelease(app as CFTypeRef);
        if err != 0 || hit.is_null() {
            return vec![];
        }
        let mut owner = 0;
        let files = if AXUIElementGetPid(hit, &mut owner) == 0 && owner == pid {
            row_files(hit)
        } else {
            vec![]
        };
        CFRelease(hit as CFTypeRef);
        files
    }

    pub unsafe fn selected_files(pid: i32, window_id: Option<u32>) -> Vec<PathBuf> {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return vec![];
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
        let Some(window) = window else {
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
        fn point_files(
            &self,
            _pid: i32,
            _window_id: Option<u32>,
            _x: f64,
            _y: f64,
        ) -> Vec<PathBuf> {
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
