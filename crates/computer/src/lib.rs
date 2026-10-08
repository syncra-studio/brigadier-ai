//! Computer use for Brigadier's workers: observe and act on the user's real desktop in the
//! background, precisely, without taking their cursor or focus.
//! The design is docs/COMPUTER-USE-PLAN.md; section numbers in comments point there.

pub mod action;
#[cfg(target_os = "macos")]
pub mod bench;
pub mod block;
pub mod cancel;
pub mod desktop;
pub mod engine;
pub mod error;
pub mod geom;
#[cfg(unix)]
pub mod harness;
pub mod record;
pub mod redact;
pub mod tree;

#[cfg(target_os = "macos")]
pub mod macos;
pub mod unsupported;

/// This system's backend.
#[cfg(target_os = "macos")]
pub fn system_desktop() -> error::CuResult<macos::MacDesktop> {
    macos::MacDesktop::new()
}

/// This system's backend.
#[cfg(not(target_os = "macos"))]
pub fn system_desktop() -> error::CuResult<unsupported::Unsupported> {
    Ok(unsupported::Unsupported)
}
