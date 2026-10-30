//! Regenerates the TypeScript protocol bindings in `apps/desktop/src/ipc/generated`.
//!
//! Usage: `cargo run -p brigadier-ipc --bin gen-ts [out-dir]`. CI runs it and fails when the
//! committed bindings differ.

use std::path::PathBuf;

use brigadier_ipc::app::{
    AppInfo, BridgeEvent, BrowserBounds, BrowserClear, BrowserDownload, BrowserEvent, BrowserFound,
    BrowserOptions, BrowserSite, BrowserText, BrowserVisit, DownloadState, HistoryFilter,
    ImportFailure, ImportFamily, ImportOutcome, ImportProgress, ImportRecord, ImportSource,
    ImportSourceProfile, ImportState, RunningChat, SitePermission, SmokeReport, UiMeasurements,
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
    BrowserFound::export_all(&config)?;
    BrowserOptions::export_all(&config)?;
    BrowserVisit::export_all(&config)?;
    BrowserText::export_all(&config)?;
    HistoryFilter::export_all(&config)?;
    BrowserDownload::export_all(&config)?;
    DownloadState::export_all(&config)?;
    BrowserSite::export_all(&config)?;
    SitePermission::export_all(&config)?;
    BrowserClear::export_all(&config)?;
    ImportSource::export_all(&config)?;
    ImportSourceProfile::export_all(&config)?;
    ImportState::export_all(&config)?;
    ImportFamily::export_all(&config)?;
    ImportRecord::export_all(&config)?;
    ImportProgress::export_all(&config)?;
    ImportOutcome::export_all(&config)?;
    ImportFailure::export_all(&config)?;
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
    // Values, not types: written after the index, which lists types only.
    std::fs::write(out_dir.join("clis.ts"), clis_ts())?;
    println!("wrote {} bindings to {}", names.len(), out_dir.display());
    Ok(())
}

/// `clis.ts`: every registered CLI's facts the app shows, from its adapter.
fn clis_ts() -> String {
    let text = |value: &str| serde_json::to_string(value).expect("a string serializes");
    let list = |values: &[&str]| {
        let values: Vec<String> = values.iter().map(|value| text(value)).collect();
        format!("[{}]", values.join(", "))
    };
    let mut out = String::from(CLIS_HEAD);
    for adapter in brigadier_providers::adapters::ADAPTERS {
        let glyph = &adapter.glyph;
        let class = glyph.class_name.map_or(String::new(), |class| {
            format!("      className: {},\n", text(class))
        });
        out.push_str(&format!(
            "  {{\n    id: {id},\n    label: {label},\n    vendor: {vendor},\n    maker: {maker},\n    \
             cliName: {cli_name},\n    installGuide: {guide},\n    usualWindows: {windows},\n    \
             glyph: {{\n      viewBox: {view_box},\n      path: {path},\n{class}    }},\n    \
             oneSkillPerMessage: {one_skill},\n    reportsCacheWrites: {cache_writes},\n    \
             subagentTools: {tools},\n  }},\n",
            id = text(adapter.kind.id()),
            label = text(adapter.label),
            vendor = text(adapter.vendor),
            maker = text(adapter.maker),
            cli_name = text(adapter.cli_name),
            guide = text(adapter.install_guide),
            windows = list(adapter.usual_windows),
            view_box = text(glyph.view_box),
            path = text(glyph.path),
            one_skill = adapter.one_skill_per_message,
            cache_writes = adapter.reports_cache_writes,
            tools = list(adapter.subagent_tools),
        ));
    }
    out.push_str("];\n");
    out
}

const CLIS_HEAD: &str = r#"// This file was generated by brigadier's gen-ts from each CLI adapter. Do not edit this file manually.
export interface CliFacts {
  /** The CLI's id (ProviderKind). */
  id: string;
  /** Product name: "Claude Code", "Codex", "OpenCode". */
  label: string;
  /** Short vendor word used in routing/report titles: "Claude", "Codex", "OpenCode". */
  vendor: string;
  /** Who makes it, on its Settings page's line. */
  maker: string;
  /** Its CLI as the Settings rows name it. */
  cliName: string;
  /** The vendor's setup docs URL. */
  installGuide: string;
  /** Usage-window ids its quota usually reports, for the fault injector. */
  usualWindows: string[];
  /** Its mark as one SVG: viewBox + path data + whether it uses its brand colour class. */
  glyph: { viewBox: string; path: string; className?: string };
  /** It runs only the first skill in a message, so the composer keeps one skill chip. */
  oneSkillPerMessage: boolean;
  /** Its usage reports cache writes; one that doesn't is judged by its cached share of input. */
  reportsCacheWrites: boolean;
  /** The tool names its native subagents start with. */
  subagentTools: string[];
}

export const CLIS: readonly CliFacts[] = [
"#;
