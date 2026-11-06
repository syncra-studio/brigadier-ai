//! What Brigadier's desktop shell does without any window toolkit, used by both shells while the app
//! moves from Tauri to Electron: the Tauri crate links it as a library, and the `napi` feature
//! builds it as the Node addon the Electron main process loads (`native`).

pub mod bridge;
pub mod browser_grants;
pub mod browser_store;
pub mod launcher;
pub mod opening;
pub mod reports;
pub mod simulator;
pub mod smoke;

// Node-API's generated glue is unsafe by nature; everything it calls here is safe Rust.
#[cfg(feature = "napi")]
#[allow(unsafe_code)]
mod native;
