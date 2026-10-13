//! Brigadier's behaviour on top of the vendored driver (`../../upstream`).
//!
//! Call [`configure_environment`] first thing, before any thread starts: it
//! points the driver's state at Brigadier's helper home. Then build the
//! registry with `registry::build`: the driver's tools, Brigadier's own in
//! place of some of them, and [`guard::install`] around every tool.

// A refusal is the driver's own `ToolResult`, returned as is to the agent.
#![allow(clippy::result_large_err)]

#[cfg(target_os = "macos")]
pub mod browser;
pub mod catalog;
pub mod focus_policy;
pub mod guard;
pub mod hotkey;
#[cfg(target_os = "macos")]
pub mod launch;
pub mod lookup;
pub mod never_front;
pub mod no_tcc;
#[cfg(target_os = "macos")]
pub mod offspace;
#[cfg(target_os = "macos")]
pub mod page;
#[cfg(target_os = "macos")]
pub mod registry;
pub mod scope;
pub mod scroll;
pub mod target_policy;
pub mod text;
pub mod tokens;

use std::path::Path;

pub use guard::install;

/// Bundle id of the app that hosts the helper; refused as a target unless
/// the session's allow list names it.
pub const HOST_BUNDLE_ID_ENV: &str = "BRIGADIER_CU_HOST_BUNDLE_ID";

/// The driver settings Brigadier fixes, as `(variable, value)`. Values
/// under `home` are filled in by [`driver_environment`].
const FIXED: &[(&str, &str)] = &[
    // Brigadier never runs agent scripts inside the helper.
    ("CUA_DRIVER_DISABLE_RUN_SCRIPT", "1"),
    // Standard permissions, and the unrestricted mode can't be switched on.
    ("CUA_DRIVER_PERMISSION_MODE", "standard"),
    ("CUA_DRIVER_DISABLE_UNRESTRICTED", "1"),
];

/// Variables that would widen what the driver does; always removed.
const CLEARED: &[&str] = &[
    "CUA_DRIVER_DANGEROUSLY_BYPASS_APPROVALS",
    "CUA_DRIVER_EXPERIMENTAL_SCRIPT",
    "CUA_DRIVER_ENABLE_LEGACY_PAGE_MUTATIONS",
    "CUA_DRIVER_POLICY_FILE",
    "CUA_DRIVER_CAPABILITY_MANIFEST_FILE",
    "CUA_DRIVER_CAPABILITY_MANIFEST_APPROVED",
    "CUA_DRIVER_SESSION_POLICY_APPROVED",
];

/// The environment the driver runs with: every place it keeps state is
/// under `home`, and the fixed settings above.
pub fn driver_environment(home: &Path) -> Vec<(&'static str, String)> {
    let under = |child: &str| home.join(child).to_string_lossy().into_owned();
    let mut env = vec![
        ("CUA_DRIVER_STATE_DIR", home.to_string_lossy().into_owned()),
        ("CUA_DRIVER_CURSOR_THEME_DIR", under("cursor-themes")),
        ("CUA_DRIVER_BROWSER_PROFILE_ROOT", under("browser-profiles")),
    ];
    env.extend(FIXED.iter().map(|(key, value)| (*key, (*value).to_owned())));
    env
}

/// Set the driver's environment for this process (see
/// [`driver_environment`]) and remove the variables that would widen it.
///
/// # Safety contract
/// Changes the process environment, so it must run before any other thread
/// exists: first thing in `main`.
pub fn configure_environment(home: &Path) {
    for key in CLEARED {
        std::env::remove_var(key);
    }
    for (key, value) in driver_environment(home) {
        std::env::set_var(key, value);
    }
}
