//! When the driver may put the user's app back in front.
//!
//! While an agent acts, the driver watches for the target app (or one the
//! action launched) coming forward and puts the user's app back. It asks
//! [`allow`] before every such restore, on every path it has: an app
//! activating during an action or a launch, a target that activated itself,
//! and a target window rising above the user's. A restore is left out when
//! it would undo something that is not the agent's doing:
//!
//! - R1: a launch that doesn't know its app's pid yet only sends back an app
//!   launched since it began, never an app that was already running, so the
//!   user's own switch to it stays. An action's own watch (a click, a key)
//!   does send back an app that was already running: one the action handed
//!   a document or a link to, which macOS brought forward;
//! - R2: nothing is sent back right after the user's own keyboard, pointer
//!   or trackpad input, unless the action itself launched the app;
//! - R3: system prompts and panels (a password or Touch ID prompt, a
//!   permission or notification alert, the Dock, Spotlight) stay in front;
//! - R4: only regular apps are sent back, never agents and panels.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// System prompts and panels that must never be sent back.
pub const SYSTEM_UI: &[&str] = &[
    "com.apple.LocalAuthentication.UIAgent",
    "com.apple.SecurityAgent",
    "com.apple.UserNotificationCenter",
    "com.apple.notificationcenterui",
    "com.apple.coreservices.uiagent",
    "com.apple.CoreLocationAgent",
    "com.apple.accessibility.universalAccessAuthWarn",
    "com.apple.loginwindow",
    "com.apple.dock",
    "com.apple.controlcenter",
    "com.apple.Spotlight",
    "com.apple.systemuiserver",
];

/// Seconds a person needs to have been idle before an activation can be
/// taken for an app taking the front rather than for their own switch.
pub const USER_INPUT_GRACE_SECS: f64 = 1.0;

/// What the policy needs to know about the app that came forward.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppFacts {
    pub bundle_id: Option<String>,
    /// A regular (Dock) app, as opposed to an agent or a background app.
    pub regular: bool,
    pub launched_at: Option<SystemTime>,
}

/// One restore the driver is about to make, in the policy's terms.
#[derive(Clone, Debug)]
pub struct Restore {
    /// The pid the action named; `None` while a launch doesn't know it yet.
    pub target_pid: Option<i32>,
    /// When the action or launch began.
    pub began: SystemTime,
    /// A launch's watch, as opposed to an action's.
    pub launch: bool,
}

/// The origin the driver and [`crate::launch`] give a launch's suppression.
const LAUNCH_ORIGIN: &str = "LaunchAppTool.";

/// Where the facts come from: the running system, or a test's fakes.
pub trait Facts: Send + Sync {
    fn app(&self, pid: i32) -> AppFacts;
    /// Seconds since the last keyboard, pointer or trackpad event from the
    /// hardware (events the helper posts to apps don't count).
    fn secs_since_user_input(&self) -> f64;
}

/// Why a restore is left out, or `None` when it may run.
pub fn refusal(
    restore: &Restore,
    app: &AppFacts,
    secs_since_user_input: f64,
) -> Option<&'static str> {
    let launched_by_action = app
        .launched_at
        .is_some_and(|launched| launched >= restore.began);
    if restore.launch && restore.target_pid.is_none() && !launched_by_action {
        return Some("already running before the action");
    }
    if app
        .bundle_id
        .as_deref()
        .is_some_and(|bundle| SYSTEM_UI.contains(&bundle))
    {
        return Some("system prompt");
    }
    if !app.regular {
        return Some("not a regular app");
    }
    if !launched_by_action && secs_since_user_input < USER_INPUT_GRACE_SECS {
        return Some("user input");
    }
    None
}

#[cfg(target_os = "macos")]
fn request_restore(request: &platform_macos::focus_steal::RestoreRequest) -> Restore {
    let elapsed = request.since.elapsed();
    Restore {
        target_pid: request.target_pid,
        began: SystemTime::now()
            .checked_sub(elapsed)
            .unwrap_or(SystemTime::UNIX_EPOCH),
        launch: request.origin.starts_with(LAUNCH_ORIGIN),
    }
}

/// Whether the driver may make `request`, with `facts` about the system.
#[cfg(target_os = "macos")]
pub fn allow(request: &platform_macos::focus_steal::RestoreRequest, facts: &dyn Facts) -> bool {
    let app = facts.app(request.activated_pid);
    match refusal(
        &request_restore(request),
        &app,
        facts.secs_since_user_input(),
    ) {
        None => true,
        Some(reason) => {
            tracing::info!(
                activated_pid = request.activated_pid,
                restore_to = request.restore_to,
                kind = ?request.kind,
                origin = request.origin,
                bundle = ?app.bundle_id,
                reason,
                "left in front"
            );
            false
        }
    }
}

/// Install the policy into the driver with the real system facts. Call
/// once at startup, before the registry is built. Returns `false` if a
/// policy was already installed.
#[cfg(target_os = "macos")]
pub fn install() -> bool {
    install_with(Arc::new(System))
}

/// [`install`] with the given facts.
#[cfg(target_os = "macos")]
pub fn install_with(facts: Arc<dyn Facts>) -> bool {
    platform_macos::focus_steal::set_restore_policy(move |request| allow(request, facts.as_ref()))
}

/// The facts of the running system.
pub struct System;

#[cfg(target_os = "macos")]
impl Facts for System {
    fn app(&self, pid: i32) -> AppFacts {
        use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
        let Some(app) =
            (unsafe { NSRunningApplication::runningApplicationWithProcessIdentifier(pid) })
        else {
            return AppFacts::default();
        };
        unsafe {
            AppFacts {
                bundle_id: app.bundleIdentifier().map(|id| id.to_string()),
                regular: app.activationPolicy() == NSApplicationActivationPolicy::Regular,
                launched_at: app.launchDate().map(|date| {
                    SystemTime::UNIX_EPOCH
                        + Duration::from_secs_f64(date.timeIntervalSince1970().max(0.0))
                }),
            }
        }
    }

    fn secs_since_user_input(&self) -> f64 {
        #[link(name = "CoreGraphics", kind = "framework")]
        extern "C" {
            fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
        }
        const HID_SYSTEM_STATE: i32 = 1;
        const ANY_INPUT_EVENT: u32 = u32::MAX;
        unsafe { CGEventSourceSecondsSinceLastEventType(HID_SYSTEM_STATE, ANY_INPUT_EVENT) }
    }
}

#[cfg(not(target_os = "macos"))]
impl Facts for System {
    fn app(&self, _pid: i32) -> AppFacts {
        AppFacts::default()
    }
    fn secs_since_user_input(&self) -> f64 {
        f64::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn regular(bundle: &str, launched_secs_ago: u64) -> AppFacts {
        AppFacts {
            bundle_id: Some(bundle.to_owned()),
            regular: true,
            launched_at: Some(SystemTime::now() - Duration::from_secs(launched_secs_ago)),
        }
    }

    fn launch(target_pid: Option<i32>, began_secs_ago: u64) -> Restore {
        Restore {
            target_pid,
            began: SystemTime::now() - Duration::from_secs(began_secs_ago),
            launch: true,
        }
    }

    fn action(target_pid: Option<i32>, began_secs_ago: u64) -> Restore {
        Restore {
            launch: false,
            ..launch(target_pid, began_secs_ago)
        }
    }

    #[test]
    fn r1_a_launch_only_sends_back_an_app_it_launched() {
        let older = regular("com.apple.TextEdit", 600);
        let fresh = regular("com.apple.TextEdit", 1);
        assert_eq!(
            refusal(&launch(None, 5), &older, 60.0),
            Some("already running before the action")
        );
        assert_eq!(refusal(&launch(None, 5), &fresh, 60.0), None);
        assert_eq!(refusal(&launch(Some(7), 5), &older, 60.0), None);
    }

    #[test]
    fn r1_an_action_sends_back_a_running_app_it_handed_a_document() {
        let preview = regular("com.apple.Preview", 600);
        assert_eq!(refusal(&action(None, 1), &preview, 60.0), None);
        // The user's own switch right then still stays (R2).
        assert_eq!(refusal(&action(None, 1), &preview, 0.2), Some("user input"));
    }

    #[test]
    fn r2_the_users_own_input_is_never_undone() {
        let older = regular("com.apple.TextEdit", 600);
        assert_eq!(
            refusal(&action(Some(7), 5), &older, 0.2),
            Some("user input")
        );
        assert_eq!(refusal(&action(Some(7), 5), &older, 1.5), None);
        // An app the action launched itself may be sent back at once.
        let fresh = regular("com.apple.TextEdit", 1);
        assert_eq!(refusal(&launch(None, 5), &fresh, 0.0), None);
        assert_eq!(refusal(&action(Some(7), 5), &fresh, 0.0), None);
    }

    #[test]
    fn r3_system_prompts_stay_in_front() {
        for bundle in SYSTEM_UI {
            assert_eq!(
                refusal(&action(Some(7), 5), &regular(bundle, 600), 60.0),
                Some("system prompt"),
                "{bundle}"
            );
            assert_eq!(
                refusal(&launch(None, 5), &regular(bundle, 1), 60.0),
                Some("system prompt"),
                "{bundle}"
            );
        }
    }

    #[test]
    fn r4_only_regular_apps_are_sent_back() {
        let agent = AppFacts {
            regular: false,
            ..regular("company.menubar", 600)
        };
        assert_eq!(
            refusal(&action(Some(7), 5), &agent, 60.0),
            Some("not a regular app")
        );
        assert_eq!(
            refusal(&action(Some(7), 5), &AppFacts::default(), 60.0),
            Some("not a regular app"),
            "an app that is gone is left alone"
        );
    }
}
