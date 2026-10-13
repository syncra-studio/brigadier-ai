//! The macOS side of Brigadier's `launch_app`: finding an app (by bundle id,
//! by name, also Finder and the other apps macOS keeps with its own
//! services, or by the full path to its bundle), starting it through
//! LaunchServices without activating it (hidden when it starts from
//! scratch), and the app the WindowServer has in front.
//!
//! The driver's own launch helpers can neither start an app hidden nor find
//! an app by path or among macOS's service apps, so these are Brigadier's.

use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::ptr::NonNull;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace, NSWorkspaceOpenConfiguration,
};
use objc2_foundation::{NSAppleEventDescriptor, NSArray, NSDictionary, NSError, NSString, NSURL};

/// A running regular (Dock) app.
#[derive(Clone, Debug)]
pub struct RunningApp {
    pub pid: i32,
    pub name: String,
    pub bundle_id: Option<String>,
}

/// The running regular apps, as AppKit has them. An app that is still
/// quitting stays listed until it is gone.
pub fn running_apps() -> Vec<RunningApp> {
    unsafe {
        let applications = NSWorkspace::sharedWorkspace().runningApplications();
        (0..applications.len())
            .filter_map(|index| applications.get(index))
            .filter(|app| {
                !app.isTerminated()
                    && app.activationPolicy() == NSApplicationActivationPolicy::Regular
            })
            .filter_map(|app| {
                let pid = app.processIdentifier();
                if pid <= 0 {
                    return None;
                }
                let bundle_id = app.bundleIdentifier().map(|value| value.to_string());
                let name = app
                    .localizedName()
                    .map(|value| value.to_string())
                    .or_else(|| bundle_id.clone())
                    .unwrap_or_else(|| format!("pid {pid}"));
                Some(RunningApp {
                    pid,
                    name,
                    bundle_id,
                })
            })
            .collect()
    }
}

/// The app in front as the WindowServer has it right now (AppKit's answer is
/// updated from notifications and can lag behind a switch that just
/// happened), else as AppKit has it.
pub fn front_pid() -> Option<i32> {
    platform_macos::input::skylight::front_process_pid()
        .or_else(platform_macos::apps::frontmost_pid)
}

// ── Finding the app ─────────────────────────────────────────────────────────

/// Where a launch found the app.
///
/// An app found through LaunchServices keeps its bundle id, never the path
/// LaunchServices gave: an app installed in a system cryptex (Safari) can't
/// be launched again from that path.
pub enum AppLocator {
    /// An `.app` bundle on disk.
    Path(String),
    /// Found by LaunchServices for this bundle id.
    BundleId(String),
}

impl AppLocator {
    /// `(what LaunchServices is handed, the bundle id when known)`.
    pub fn app_ref_and_bundle_id(self) -> (String, Option<String>) {
        match self {
            AppLocator::Path(path) => {
                let bundle_id = crate::lookup::bundle_id_at(std::path::Path::new(&path));
                (path, bundle_id)
            }
            AppLocator::BundleId(bundle_id) => (bundle_id.clone(), Some(bundle_id)),
        }
    }
}

/// Whether LaunchServices knows an app with `bundle_id`.
pub fn bundle_installed(bundle_id: &str) -> bool {
    unsafe {
        NSWorkspace::sharedWorkspace()
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))
            .is_some()
    }
}

/// The folders searched for `<name>.app`, in order: the Applications
/// folders, then Finder and the other apps macOS keeps with its own services.
pub fn search_roots(home: &str) -> Vec<String> {
    vec![
        "/Applications".to_owned(),
        "/System/Applications".to_owned(),
        "/System/Applications/Utilities".to_owned(),
        "/Applications/Utilities".to_owned(),
        format!("{home}/Applications"),
        format!("{home}/Applications/Chrome Apps.localized"),
        "/System/Library/CoreServices".to_owned(),
        "/System/Library/CoreServices/Applications".to_owned(),
    ]
}

/// The app `name` names: the full path to an `.app` bundle wherever it
/// lives, an app's name in [`search_roots`], or a bundle id given as a name.
pub fn locate_by_name(name: &str) -> Option<AppLocator> {
    if name.starts_with('/') && name.ends_with(".app") && std::path::Path::new(name).is_dir() {
        return Some(AppLocator::Path(name.to_owned()));
    }
    let app_name = if name.ends_with(".app") {
        name.to_owned()
    } else {
        format!("{name}.app")
    };
    let home = std::env::var("HOME").unwrap_or_default();
    for root in search_roots(&home) {
        let path = format!("{root}/{app_name}");
        if std::path::Path::new(&path).is_dir() {
            return Some(AppLocator::Path(path));
        }
    }
    bundle_installed(name).then(|| AppLocator::BundleId(name.to_owned()))
}

// ── Launching ───────────────────────────────────────────────────────────────

/// What to launch and how.
pub struct Launch<'a> {
    pub bundle_id: Option<&'a str>,
    pub name: Option<&'a str>,
    pub urls: &'a [String],
    pub arguments: &'a [String],
    pub environment: &'a HashMap<String, String>,
    pub creates_new_instance: bool,
}

impl Launch<'_> {
    /// Nothing but the app itself: no urls, arguments, environment or new
    /// instance.
    fn plain(&self) -> bool {
        self.urls.is_empty()
            && self.arguments.is_empty()
            && self.environment.is_empty()
            && !self.creates_new_instance
    }
}

/// Launch the app without activating it and return its pid. An app that
/// starts a new process for urls, arguments or a new instance starts
/// hidden and is shown, still without activating it, once it has finished
/// launching.
pub fn launch(request: &Launch<'_>) -> anyhow::Result<i32> {
    let (app_ref, bundle_id, label) = match (request.bundle_id, request.name) {
        (Some(bundle_id), _) => (
            bundle_id.to_owned(),
            Some(bundle_id.to_owned()),
            bundle_id.to_owned(),
        ),
        (None, Some(name)) => {
            let (app_ref, bundle_id) = locate_by_name(name)
                .ok_or_else(|| anyhow::anyhow!("Could not locate app with name '{name}'"))?
                .app_ref_and_bundle_id();
            (app_ref, bundle_id, format!("'{name}'"))
        }
        (None, None) => anyhow::bail!("Provide either bundle_id or name"),
    };
    let config = if request.plain() {
        OpenConfig {
            apple_event_bundle_id: bundle_id,
            ..Default::default()
        }
    } else {
        // With urls, LaunchServices sends its own open-documents event; an
        // open-application event on top makes it fail for apps installed in
        // a system cryptex (Safari).
        OpenConfig {
            arguments: request.arguments.to_vec(),
            environment: request.environment.clone(),
            creates_new_instance: request.creates_new_instance,
            hides: starts_fresh(bundle_id.as_deref(), request.creates_new_instance),
            apple_event_bundle_id: if request.urls.is_empty() {
                bundle_id
            } else {
                None
            },
        }
    };
    let urls = (!request.urls.is_empty()).then_some(request.urls);
    let running = open(&app_ref, urls, &config)
        .map_err(|error| anyhow::anyhow!("Failed to launch {label}: {error}"))?;
    if config.hides {
        show_without_activating(&running);
    }
    Ok(unsafe { running.processIdentifier() })
}

/// Whether a launch starts a new process: the app isn't running yet, or a
/// new instance was asked for. Such a launch starts hidden, because apps
/// bring themselves to the front as they finish launching.
fn starts_fresh(bundle_id: Option<&str>, creates_new_instance: bool) -> bool {
    creates_new_instance
        || bundle_id.is_some_and(|bundle_id| {
            !running_apps()
                .iter()
                .any(|app| app.bundle_id.as_deref() == Some(bundle_id))
        })
}

/// Show an app started hidden once it has finished launching, without
/// activating it: its windows come back where they opened (behind the
/// user's work), and an activation it asks for later is sent back.
fn show_without_activating(app: &NSRunningApplication) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !unsafe { app.isFinishedLaunching() } && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    // Let the app's own launch-time activation pass while it is still hidden.
    std::thread::sleep(Duration::from_millis(200));
    unsafe { app.unhide() };
}

/// The `NSWorkspaceOpenConfiguration` a launch uses. Always
/// `activates = false` and `addsToRecentItems = false`.
#[derive(Default)]
struct OpenConfig {
    arguments: Vec<String>,
    environment: HashMap<String, String>,
    creates_new_instance: bool,
    /// Sent as an open-application event, so an app starting from scratch
    /// (or restoring its state) reliably opens its window.
    apple_event_bundle_id: Option<String>,
    hides: bool,
}

/// A wedged LaunchServices call fails after this long instead of hanging.
const COMPLETION_TIMEOUT: Duration = Duration::from_secs(30);

type Completion = Result<Retained<NSRunningApplication>, String>;

/// Open the app `app_ref` (a bundle id, an `.app` path or a URL), handing it
/// `urls` when given, and wait for LaunchServices' answer.
fn open(
    app_ref: &str,
    urls: Option<&[String]>,
    config: &OpenConfig,
) -> Result<Retained<NSRunningApplication>, String> {
    let workspace = unsafe { NSWorkspace::sharedWorkspace() };
    let app_url = application_url(&workspace, app_ref)?;
    let configuration = configuration(config);
    let (tx, rx) = mpsc::sync_channel::<Completion>(1);
    // AppKit calls the completion block once, on its own queue; the block
    // owns the only other handle to the sender.
    #[allow(clippy::arc_with_non_send_sync)]
    let tx = Arc::new(Mutex::new(Some(tx)));
    let block = RcBlock::new(move |app: *mut NSRunningApplication, error: *mut NSError| {
        let result = unsafe {
            if !error.is_null() {
                Err(format!(
                    "NSWorkspace launch failed: {}",
                    (*error).localizedDescription()
                ))
            } else {
                NonNull::new(app)
                    .and_then(|app| Retained::retain(app.as_ptr()))
                    .ok_or_else(no_app)
            }
        };
        if let Some(sender) = tx.lock().ok().and_then(|mut slot| slot.take()) {
            let _ = sender.send(result);
        }
    });
    unsafe {
        match urls {
            None => workspace.openApplicationAtURL_configuration_completionHandler(
                &app_url,
                &configuration,
                Some(&block),
            ),
            Some(urls) => {
                let urls = urls
                    .iter()
                    .map(|url| file_or_url(url))
                    .collect::<Result<Vec<_>, _>>()?;
                workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
                    &NSArray::from_vec(urls),
                    &app_url,
                    &configuration,
                    Some(&block),
                )
            }
        }
    }
    match rx.recv_timeout(COMPLETION_TIMEOUT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "NSWorkspace launch did not complete within {COMPLETION_TIMEOUT:?}"
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(no_app()),
    }
}

fn no_app() -> String {
    "NSWorkspace launch returned no NSRunningApplication and no NSError".to_owned()
}

fn configuration(config: &OpenConfig) -> Retained<NSWorkspaceOpenConfiguration> {
    unsafe {
        let configuration = NSWorkspaceOpenConfiguration::configuration();
        configuration.setActivates(false);
        configuration.setAddsToRecentItems(false);
        configuration.setCreatesNewApplicationInstance(config.creates_new_instance);
        configuration.setHides(config.hides);
        if !config.arguments.is_empty() {
            let arguments: Vec<Retained<NSString>> = config
                .arguments
                .iter()
                .map(|argument| NSString::from_str(argument))
                .collect();
            configuration.setArguments(&NSArray::from_vec(arguments));
        }
        // Only the caller's own variables, never this process's
        // environment: it may hold secrets.
        if !config.environment.is_empty() {
            let keys: Vec<Retained<NSString>> = config
                .environment
                .keys()
                .map(|key| NSString::from_str(key))
                .collect();
            let values: Vec<Retained<NSString>> = config
                .environment
                .values()
                .map(|value| NSString::from_str(value))
                .collect();
            let key_refs: Vec<&NSString> = keys.iter().map(|key| key.as_ref()).collect();
            configuration.setEnvironment(&NSDictionary::from_vec(&key_refs, values));
        }
        if let Some(bundle_id) = config.apple_event_bundle_id.as_deref() {
            if !bundle_id.is_empty() {
                configuration.setAppleEvent(Some(&open_application_event(bundle_id)));
            }
        }
        configuration
    }
}

/// LaunchServices' own URL for a bundle id (kept exactly as given, see
/// [`AppLocator`]), else the path or URL `app_ref` is.
fn application_url(workspace: &NSWorkspace, app_ref: &str) -> Result<Retained<NSURL>, String> {
    let looks_like_bundle_id =
        !app_ref.contains('/') && !app_ref.contains("://") && app_ref.contains('.');
    if looks_like_bundle_id {
        if let Some(url) =
            unsafe { workspace.URLForApplicationWithBundleIdentifier(&NSString::from_str(app_ref)) }
        {
            return Ok(url);
        }
    }
    file_or_url(app_ref)
}

/// A URL for a scheme (`https:`, `file:`, `about:`, an app's own), a file
/// URL for a path.
fn file_or_url(value: &str) -> Result<Retained<NSURL>, String> {
    if value.is_empty() {
        return Err("invalid url: empty".to_owned());
    }
    let looks_like_url = value.contains(':') && !value.starts_with('/') && !value.starts_with('~');
    let string = NSString::from_str(value);
    if looks_like_url {
        unsafe { NSURL::URLWithString(&string) }.ok_or_else(|| format!("invalid url: {value}"))
    } else {
        Ok(unsafe { NSURL::fileURLWithPath(&string) })
    }
}

/// An open-application (`aevt/oapp`) event addressed to `bundle_id`.
fn open_application_event(bundle_id: &str) -> Retained<NSAppleEventDescriptor> {
    use objc2::rc::Allocated;
    use objc2::{msg_send_id, ClassType};

    const fn fourcc(code: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*code)
    }
    const CORE_EVENT_CLASS: u32 = fourcc(b"aevt");
    const OPEN_APPLICATION: u32 = fourcc(b"oapp");
    const AUTO_GENERATE_RETURN_ID: i16 = -1;
    const ANY_TRANSACTION_ID: i32 = 0;

    unsafe {
        let target =
            NSAppleEventDescriptor::descriptorWithBundleIdentifier(&NSString::from_str(bundle_id));
        let alloc: Allocated<NSAppleEventDescriptor> = NSAppleEventDescriptor::alloc();
        msg_send_id![
            alloc,
            initWithEventClass: CORE_EVENT_CLASS,
            eventID: OPEN_APPLICATION,
            targetDescriptor: &*target,
            returnID: AUTO_GENERATE_RETURN_ID,
            transactionID: ANY_TRANSACTION_ID,
        ]
    }
}

// ── Sending an app back ─────────────────────────────────────────────────────

/// Make `pid` the front process again at once, through the WindowServer
/// (back within ~50 ms, where an AppKit activation took about a second).
/// `false` when the process or the private calls aren't available.
pub fn set_front_pid(pid: i32) -> bool {
    type SetFrontProcess = unsafe extern "C" fn(*const c_void, u32, u32) -> i32;
    type GetProcessForPid = unsafe extern "C" fn(libc::pid_t, *mut c_void) -> i32;
    static SET_FRONT: OnceLock<Option<SetFrontProcess>> = OnceLock::new();
    static PROCESS_FOR_PID: OnceLock<Option<GetProcessForPid>> = OnceLock::new();
    let set_front = *SET_FRONT.get_or_init(|| {
        symbol(b"SLPSSetFrontProcessWithOptions\0")
            .map(|ptr| unsafe { std::mem::transmute::<*mut c_void, SetFrontProcess>(ptr) })
    });
    let process_for_pid = *PROCESS_FOR_PID.get_or_init(|| {
        symbol(b"GetProcessForPID\0")
            .map(|ptr| unsafe { std::mem::transmute::<*mut c_void, GetProcessForPid>(ptr) })
    });
    let (Some(set_front), Some(process_for_pid)) = (set_front, process_for_pid) else {
        return false;
    };
    let mut psn = [0u8; 8];
    if unsafe { process_for_pid(pid, psn.as_mut_ptr().cast()) } != 0 {
        return false;
    }
    // Window 0 with kCPSNoWindows (0x400): front the process, leave its
    // windows' order alone.
    unsafe { set_front(psn.as_ptr().cast(), 0, 0x400) == 0 }
}

/// A symbol from SkyLight (loaded once) or any image already loaded.
fn symbol(name: &[u8]) -> Option<*mut c_void> {
    static LOADED: OnceLock<()> = OnceLock::new();
    LOADED.get_or_init(|| unsafe {
        libc::dlopen(
            b"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight\0"
                .as_ptr()
                .cast::<c_char>(),
            libc::RTLD_LAZY | libc::RTLD_GLOBAL,
        );
    });
    let pointer = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr().cast::<c_char>()) };
    (!pointer.is_null()).then_some(pointer)
}

// ── Windows ─────────────────────────────────────────────────────────────────

/// The window id of the app's main window, through Accessibility.
pub fn main_window_id(pid: i32) -> Option<u32> {
    use platform_macos::ax::bindings::{
        ax_get_window_id, copy_element_attr, AXUIElementCreateApplication,
    };
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let window = copy_element_attr(app, "AXMainWindow");
        core_foundation::base::CFRelease(app as _);
        let window = window?;
        let id = ax_get_window_id(window);
        core_foundation::base::CFRelease(window as _);
        id
    }
}
