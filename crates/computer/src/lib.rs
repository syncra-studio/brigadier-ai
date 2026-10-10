//! Computer use for Brigadier's workers: observe and act on the user's real desktop in the
//! background, precisely, without taking their cursor or focus.
//! The design is docs/COMPUTER-USE-PLAN.md; section numbers in comments point there.

pub mod action;
#[cfg(all(target_os = "macos", feature = "engine"))]
pub mod bench;
pub mod block;
pub mod cancel;
pub mod cdp;
pub mod cursor;
pub mod desktop;
pub mod engine;
pub mod error;
pub mod geom;
#[cfg(unix)]
pub mod harness;
#[cfg(all(target_os = "macos", feature = "engine"))]
pub mod helper;
pub mod launch;
pub mod record;
pub mod redact;
pub mod suite;
pub mod suite_quirks;
#[cfg(all(target_os = "macos", feature = "engine"))]
pub mod suite_quirks_run;
#[cfg(all(target_os = "macos", feature = "engine"))]
pub mod suite_run;
pub mod suite_web;
pub mod tree;

#[cfg(unix)]
pub mod client;
#[cfg(all(target_os = "macos", feature = "engine"))]
pub mod macos;
pub mod unsupported;
pub mod web_fixture;
pub mod wire;

/// This system's backend.
#[cfg(all(target_os = "macos", feature = "engine"))]
pub fn system_desktop() -> error::CuResult<macos::MacDesktop> {
    macos::MacDesktop::new()
}

/// This system's backend.
#[cfg(not(target_os = "macos"))]
pub fn system_desktop() -> error::CuResult<unsupported::Unsupported> {
    Ok(unsupported::Unsupported)
}
