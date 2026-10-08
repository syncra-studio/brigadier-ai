//! Computer use for Brigadier's workers: observe and act on the user's real desktop in the
//! background, precisely, without taking their cursor or focus.
//! The design is docs/COMPUTER-USE-PLAN.md; section numbers in comments point there.

pub mod action;
pub mod block;
pub mod cancel;
pub mod desktop;
pub mod engine;
pub mod error;
pub mod geom;
pub mod record;
pub mod redact;
pub mod tree;

pub mod unsupported;

/// This system's backend.
pub fn system_desktop() -> error::CuResult<unsupported::Unsupported> {
    Ok(unsupported::Unsupported)
}
