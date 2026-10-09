//! `brigadier-computer`: the computer-use engine's development and fixture harness.
//!
//!   brigadier-computer apps
//!   brigadier-computer observe <window id or title> [always|never|auto]
//!   brigadier-computer run <script.json> [--out <dir>]
//!   brigadier-computer serve --socket <path> --token-file <path> [--parent <pid>]
//!   brigadier-computer bench [--out <dir>] [--quick] [--no-foreground] [--cursor] [--replay <dir>]
//!   brigadier-computer suite tasks | setup <task> <dir> [--seed n] | check <dir> [--records f] [--report f] [--offline]
//!                            | teardown <dir> | scripted <out> [task…] | watch <out.jsonl>
//!
//! `serve` is the long-lived helper the daemon talks to; the rest is a development and fixture
//! harness. `bench --cursor` draws the agent cursor over the bench's actions, and the unlisted
//! `cursor-demo` draws two cursors for a few seconds without acting on anything. Workers reach the engine through the daemon (docs/COMPUTER-USE-PLAN.md §4.1).

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use std::path::PathBuf;

    use anyhow::{Context, anyhow, bail};
    use brigadier_computer::{bench, harness, helper};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    // The helper answers without grants, so the user can be walked through giving them.
    if args.first().map(String::as_str) == Some("serve") {
        let socket = flag("--socket").context("--socket <path>")?;
        let token_file = flag("--token-file").context("--token-file <path>")?;
        let parent = match flag("--parent") {
            Some(p) => Some(
                p.to_str()
                    .and_then(|s| s.parse::<i32>().ok())
                    .context("--parent <pid>")?,
            ),
            None => None,
        };
        return brigadier_computer::helper::serve(helper::Options {
            socket,
            token_file,
            parent,
        });
    }
    // Drawing needs no grant.
    if args.first().map(String::as_str) == Some("cursor-demo") {
        let mtm = objc2::MainThreadMarker::new().context("the main thread")?;
        brigadier_computer::macos::overlay::run_with_overlay(mtm, |overlay| {
            brigadier_computer::macos::overlay::demo(&*overlay)
        });
    }
    // Listing and checking read files only.
    if args.first().map(String::as_str) == Some("suite") {
        use brigadier_computer::suite;
        match args.get(1).map(String::as_str) {
            Some("tasks") => {
                let all: Vec<_> = suite::all_tasks()
                    .map(|t| {
                        let mut v = serde_json::to_value(t).unwrap_or_default();
                        v["brief"] = serde_json::json!(t.brief());
                        v
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&all)?);
                return Ok(());
            }
            Some("check") => {
                let dir = PathBuf::from(args.get(2).context("check <dir>")?);
                let v = brigadier_computer::suite_run::check_dir(
                    &dir,
                    flag("--records").as_deref(),
                    flag("--report").as_deref(),
                    !args.iter().any(|a| a == "--offline"),
                )?;
                println!("{}", serde_json::to_string_pretty(&v)?);
                std::process::exit(if v.pass { 0 } else { 1 });
            }
            Some("teardown") => {
                let dir = PathBuf::from(args.get(2).context("teardown <dir>")?);
                let prep: suite::Prepared =
                    serde_json::from_slice(&std::fs::read(dir.join("setup.json"))?)?;
                brigadier_computer::suite_run::teardown(&prep);
                return Ok(());
            }
            _ => {}
        }
    }
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
        Some("bench") => {
            let out = flag("--out").unwrap_or_else(|| PathBuf::from("target/computer-bench"));
            let quick = args.iter().any(|a| a == "--quick");
            // P2f raises a window and takes the front; off while someone uses the Mac.
            let foreground = !args.iter().any(|a| a == "--no-foreground");
            let replay = flag("--replay");
            if args.iter().any(|a| a == "--cursor") {
                // AppKit on this thread draws the cursor; the bench runs on another.
                let mtm = objc2::MainThreadMarker::new().context("the main thread")?;
                brigadier_computer::macos::overlay::run_with_overlay(mtm, move |overlay| {
                    let run = desktop().and_then(|d| {
                        bench::run(d, &out, quick, foreground, Some(overlay), replay.as_deref())
                    });
                    match run {
                        Ok(true) => 0,
                        Ok(false) => 1,
                        Err(e) => {
                            eprintln!("brigadier-computer: {e:#}");
                            1
                        }
                    }
                });
            }
            let ok = bench::run(desktop()?, &out, quick, foreground, None, replay.as_deref())?;
            if !ok {
                std::process::exit(1);
            }
        }
        Some("suite") => {
            use brigadier_computer::{suite, suite_run};
            match args.get(1).map(String::as_str) {
                Some("setup") => {
                    let task = args
                        .get(2)
                        .and_then(|t| suite::task(t))
                        .context("setup <task> <dir>")?;
                    let dir = PathBuf::from(args.get(3).context("setup <task> <dir>")?);
                    let seed = flag("--seed")
                        .and_then(|s| s.to_str()?.parse().ok())
                        .unwrap_or(1);
                    let prep = suite_run::setup(&mut desktop()?, task, &dir, seed)?;
                    println!("{}", serde_json::to_string(&prep)?);
                }
                Some("scripted") => {
                    let out = PathBuf::from(args.get(2).context("scripted <out> [task…]")?);
                    if !suite_run::scripted(desktop()?, &out, &args[3..])? {
                        std::process::exit(1);
                    }
                }
                Some("watch") => {
                    let out = PathBuf::from(args.get(2).context("watch <out.jsonl>")?);
                    suite_run::watch(desktop()?, &out)?;
                }
                _ => bail!("suite tasks | setup | check | teardown | scripted | watch"),
            }
        }
        _ => {
            eprintln!(
                "usage: brigadier-computer apps | observe <window> [always|never|auto] | run <script> | serve --socket <p> --token-file <p> [--parent <pid>] | bench [--out <dir>] [--quick] [--no-foreground] [--cursor] [--replay <dir>]"
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
