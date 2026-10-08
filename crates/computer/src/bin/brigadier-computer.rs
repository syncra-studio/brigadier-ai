//! `brigadier-computer`: the computer-use engine's development and fixture harness.
//!
//!   brigadier-computer apps
//!   brigadier-computer observe <window id or title> [always|never|auto]
//!   brigadier-computer run <script.json> [--out <dir>]
//!   brigadier-computer serve --socket <path> --token-file <path>
//!   brigadier-computer bench [--out <dir>] [--quick]
//!
//! Workers reach the engine through the daemon instead (docs/COMPUTER-USE-PLAN.md §4.1).

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use std::path::PathBuf;

    use anyhow::{Context, anyhow, bail};
    use brigadier_computer::{bench, harness};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    let (ax, screen) = brigadier_computer::macos::permissions();
    if !ax || !screen {
        bail!(
            "missing permissions: accessibility {ax}, screen recording {screen}; grant them to the app or terminal that runs this"
        );
    }
    let desktop = || brigadier_computer::system_desktop().map_err(|e| anyhow!("{e}"));
    match args.first().map(String::as_str) {
        Some("apps") => {
            let mut engine = harness::new_engine(desktop()?);
            print!("{}", engine.apps_text().map_err(|e| anyhow!("{e}"))?);
        }
        Some("observe") => {
            let mut engine = harness::new_engine(desktop()?);
            let target = args.get(1).context("observe <window>")?;
            let window = match target.parse::<u32>() {
                Ok(id) => serde_json::json!(id),
                Err(_) => serde_json::json!(target),
            };
            let shot = args.get(2).cloned().unwrap_or_else(|| "auto".into());
            let script =
                serde_json::json!({"steps": [{"observe": {"window": window, "screenshot": shot}}]});
            harness::run_script(
                &mut engine,
                &script,
                &harness::out_dir(flag("--out"), "observe"),
            )?;
        }
        Some("run") => {
            let mut engine = harness::new_engine(desktop()?);
            let path = args.get(1).context("run <script.json>")?;
            let script: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
            harness::run_script(
                &mut engine,
                &script,
                &harness::out_dir(flag("--out"), "run"),
            )?;
        }
        Some("serve") => {
            let socket = flag("--socket").context("--socket <path>")?;
            let token = flag("--token-file").context("--token-file <path>")?;
            harness::serve(harness::new_engine(desktop()?), &socket, &token)?;
        }
        Some("bench") => {
            let out = flag("--out").unwrap_or_else(|| PathBuf::from("target/computer-bench"));
            let quick = args.iter().any(|a| a == "--quick");
            let ok = bench::run(desktop()?, &out, quick)?;
            if !ok {
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!(
                "usage: brigadier-computer apps | observe <window> [always|never|auto] | run <script> | serve --socket <p> --token-file <p> | bench [--out <dir>] [--quick]"
            );
            std::process::exit(2);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("brigadier-computer: computer use isn't available on this system yet");
    std::process::exit(2);
}
