//! Platform-neutral MCP projection and authenticated local transport.
//! Kept testable independently of the macOS desktop integration harness.

#[path = "lean.rs"]
pub mod lean;
pub mod peer_auth;
#[cfg(not(target_os = "macos"))]
pub mod process;
pub mod transport;
