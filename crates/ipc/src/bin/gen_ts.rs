//! Regenerates the TypeScript protocol bindings in `apps/desktop/src/ipc/generated`.
//!
//! Usage: `cargo run -p brigadier-ipc --bin gen-ts [out-dir]`. CI runs it and fails when the
//! committed bindings differ.

use std::path::PathBuf;

use brigadier_ipc::app::{
    AppInfo, BridgeEvent, BrowserBounds, BrowserEvent, BrowserOptions, RunningChat, SmokeReport,
    UiMeasurements,
};
use brigadier_ipc::metrics::Diagnostics;
use brigadier_ipc::protocol::{ClientFrame, ServerFrame};
use ts_rs::{Config, TS};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src/ipc/generated")
        });
    // Start clean so bindings for removed types disappear.
    if out_dir.exists() {
        std::fs::remove_dir_all(&out_dir)?;
    }
    std::fs::create_dir_all(&out_dir)?;

    // Sequence numbers and byte counts stay far below 2^53, so plain numbers are exact.
    let config = Config::new()
        .with_out_dir(&out_dir)
        .with_large_int("number")
        .with_import_extension(None::<String>);
    ClientFrame::export_all(&config)?;
    ServerFrame::export_all(&config)?;
    Diagnostics::export_all(&config)?;
    brigadier_core::DomainEvent::export_all(&config)?;
    BridgeEvent::export_all(&config)?;
    BrowserEvent::export_all(&config)?;
    BrowserBounds::export_all(&config)?;
    BrowserOptions::export_all(&config)?;
    RunningChat::export_all(&config)?;
    AppInfo::export_all(&config)?;
    UiMeasurements::export_all(&config)?;
    SmokeReport::export_all(&config)?;

    let mut names: Vec<String> = std::fs::read_dir(&out_dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".ts").map(str::to_owned))
        .collect();
    names.sort();
    let index: String = names
        .iter()
        .map(|name| format!("export type {{ {name} }} from \"./{name}\";\n"))
        .collect();
    std::fs::write(out_dir.join("index.ts"), index)?;
    println!("wrote {} bindings to {}", names.len(), out_dir.display());
    Ok(())
}
