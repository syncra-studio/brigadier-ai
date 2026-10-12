//! Activity store core: computer use sessions, their event log, redaction
//! and retention. Pure logic, no I/O; the daemon owns persistence.
//!
//! The helper daemon is the single writer of every type here; clients (the
//! Brigadier app, the CLI) are projections that read records and events and
//! send user operations.

pub mod host;
pub mod model;
pub mod redact;
pub mod reducer;
pub mod retention;
pub mod store;
pub mod thumbnail;
