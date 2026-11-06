//! Hosts the launch smoke check (`brigadier-app --smoke`). The verdict is
//! `brigadier_shell_native::smoke`: this prints it and ends the run.

use std::time::Duration;

use brigadier_ipc::app::SmokeReport;
pub use brigadier_shell_native::smoke::{REPORT_ENV, WATCHDOG, evaluate};
use tauri::AppHandle;

use crate::shell;

/// Prints the report, writes it to `BRIGADIER_SMOKE_REPORT` if set, then quits.
pub fn finish(app: &AppHandle, report: &SmokeReport) {
    let json = serde_json::to_string_pretty(report)
        .unwrap_or_else(|err| format!("{{\"error\":\"{err}\"}}"));
    println!("{json}");
    if let Some(path) = std::env::var_os(REPORT_ENV)
        && let Err(err) = std::fs::write(&path, &json)
    {
        eprintln!("could not write the smoke report: {err}");
    }
    shell::quit(app, if report.passed { 0 } else { 1 });
}

/// Fails the run if it has not finished in time.
pub fn start_watchdog(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(WATCHDOG).await;
        eprintln!("smoke check timed out after {WATCHDOG:?}");
        if let Some(path) = std::env::var_os(REPORT_ENV) {
            let _ = std::fs::write(&path, "{\"passed\":false,\"error\":\"timed out\"}");
        }
        shell::quit(&app, 2);
        // If even the quit hangs, leave anyway.
        tokio::time::sleep(Duration::from_secs(10)).await;
        std::process::exit(2);
    });
}
