//! Each of the driver's restore paths asks Brigadier's focus policy first.
//! Its own test binary: the policy is installed once per process. Fake app
//! facts; nothing is activated (the paths are asked what they would do).
#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use cu_layer::focus_policy::{self, AppFacts, Facts};
use platform_macos::focus_steal::{self, FocusStealPreventer};
use platform_macos::key_focus_guard;

#[derive(Default)]
struct World {
    apps: HashMap<i32, AppFacts>,
    secs_since_user_input: f64,
}

struct FakeFacts;

fn world() -> &'static Mutex<World> {
    static WORLD: OnceLock<Mutex<World>> = OnceLock::new();
    WORLD.get_or_init(|| {
        Mutex::new(World {
            secs_since_user_input: 60.0,
            ..Default::default()
        })
    })
}

impl Facts for FakeFacts {
    fn app(&self, pid: i32) -> AppFacts {
        world()
            .lock()
            .unwrap()
            .apps
            .get(&pid)
            .cloned()
            .unwrap_or_default()
    }
    fn secs_since_user_input(&self) -> f64 {
        world().lock().unwrap().secs_since_user_input
    }
}

/// The tests share the process-wide policy and suppression table, so they
/// run one at a time.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    static INSTALL: OnceLock<()> = OnceLock::new();
    INSTALL.get_or_init(|| {
        assert!(focus_policy::install_with(Arc::new(FakeFacts)));
    });
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn set_app(pid: i32, bundle: &str, regular: bool, launched_secs_ago: u64) {
    world().lock().unwrap().apps.insert(
        pid,
        AppFacts {
            bundle_id: Some(bundle.to_owned()),
            regular,
            launched_at: Some(SystemTime::now() - Duration::from_secs(launched_secs_ago)),
        },
    );
}

fn set_idle(secs: f64) {
    world().lock().unwrap().secs_since_user_input = secs;
}

// Fake pids, far from real ones.
const USER_APP: i32 = 990_001;
const TARGET: i32 = 990_002;
const OTHER: i32 = 990_003;
const PROMPT: i32 = 990_004;
const AGENT: i32 = 990_005;

#[test]
fn path_1_an_activation_during_an_action_asks_the_policy() {
    let _serial = serial();
    set_idle(60.0);
    set_app(TARGET, "com.apple.TextEdit", true, 600);
    set_app(PROMPT, "com.apple.SecurityAgent", true, 600);
    set_app(AGENT, "company.menubar", false, 600);

    let lease = focus_steal::begin_suppression(Some(TARGET), USER_APP, "test.action");
    assert_eq!(
        FocusStealPreventer::restores_for_activation(TARGET),
        vec![USER_APP]
    );
    set_idle(0.2);
    assert!(
        FocusStealPreventer::restores_for_activation(TARGET).is_empty(),
        "R2: right after the user's input"
    );
    set_idle(60.0);
    drop(lease);

    for pid in [PROMPT, AGENT] {
        let lease = focus_steal::begin_suppression(Some(pid), USER_APP, "test.action");
        assert!(
            FocusStealPreventer::restores_for_activation(pid).is_empty(),
            "R3/R4: {pid}"
        );
        drop(lease);
    }
}

#[test]
fn path_1_a_launch_only_sends_back_what_it_launched() {
    let _serial = serial();
    set_idle(0.0);
    set_app(OTHER, "com.apple.Notes", true, 600);
    // The driver's launch path: a wildcard entry while the pid is unknown.
    let lease = focus_steal::begin_suppression(None, USER_APP, "LaunchAppTool.pre");
    std::thread::sleep(Duration::from_millis(5));
    assert!(
        FocusStealPreventer::restores_for_activation(OTHER).is_empty(),
        "R1: an app that was already running is the user's"
    );
    // An app launched by this launch, even right after the user typed.
    set_app(TARGET, "com.apple.TextEdit", true, 0);
    assert_eq!(
        FocusStealPreventer::restores_for_activation(TARGET),
        vec![USER_APP]
    );
    drop(lease);
    set_idle(60.0);
}

#[test]
fn path_1_an_action_sends_back_a_running_app_it_handed_off_to() {
    let _serial = serial();
    set_idle(60.0);
    set_app(OTHER, "com.apple.Preview", true, 600);
    // The driver's watch around an action: a wildcard entry for any app but the user's.
    let lease = focus_steal::begin_suppression(None, USER_APP, "WindowChangeDetector.snapshot");
    assert_eq!(
        FocusStealPreventer::restores_for_activation(OTHER),
        vec![USER_APP],
        "an app that was already running and came forward from the action"
    );
    set_idle(0.2);
    assert!(
        FocusStealPreventer::restores_for_activation(OTHER).is_empty(),
        "R2: the user's own switch stays"
    );
    drop(lease);
    set_idle(60.0);
}

#[test]
fn path_2_a_self_activated_target_asks_the_policy() {
    let _serial = serial();
    set_app(TARGET, "com.apple.TextEdit", true, 600);
    set_idle(0.2);
    assert!(
        !key_focus_guard::would_revert_self_activation(Some(TARGET), TARGET, USER_APP),
        "R2 holds on the self-activation path"
    );
    set_app(TARGET, "com.apple.SecurityAgent", true, 600);
    set_idle(60.0);
    assert!(
        !key_focus_guard::would_revert_self_activation(Some(TARGET), TARGET, USER_APP),
        "R3 holds on the self-activation path"
    );
}

#[test]
fn path_3_a_raised_window_asks_the_policy() {
    let _serial = serial();
    set_app(TARGET, "com.apple.TextEdit", true, 600);
    set_idle(60.0);
    assert!(key_focus_guard::order_restore_allowed(TARGET, USER_APP));
    set_idle(0.2);
    assert!(!key_focus_guard::order_restore_allowed(TARGET, USER_APP));
    set_idle(60.0);
    set_app(TARGET, "company.menubar", false, 600);
    assert!(!key_focus_guard::order_restore_allowed(TARGET, USER_APP));
}
