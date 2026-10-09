//! The hard-surface tasks' macOS side (Phase 5, stream B): building and opening their fixtures
//! in the background, and their scripted solutions.
//!
//! - `quirk-pad` is plain `swiftc` like the Phase 4 fixtures.
//! - `catalyst-pad` is UIKit built for the Mac with `swiftc -target <arch>-apple-ios<ver>-macabi`
//!   against the macOS SDK's iOS support frameworks.
//! - `electron-pad` runs on a pinned Electron, installed once from npm into
//!   `target/computer-suite/electron-runtime` (its own folder), with a Node-API addon built by
//!   clang that orders the window behind every other window.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::action::Screenshot;
use crate::desktop::Desktop as _;
use crate::macos::MacDesktop;
use crate::suite::{Prepared, Setup, Task};
use crate::suite_quirks::{CATALYST, ELECTRON, QUIRK, SAVED};
use crate::suite_run::{self, Script, click, set};

/// The Electron the fixture runs on, pinned.
pub const ELECTRON_VERSION: &str = "44.7.0";
/// The iOS version Catalyst builds target.
const CATALYST_IOS: &str = "18.0";

fn suite_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/computer-suite")
}

fn fixture_src(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Extra `swiftc` arguments for a fixture: the Mac Catalyst target for `catalyst-pad`.
pub fn swiftc_args(name: &str) -> Result<Vec<String>> {
    if name != CATALYST {
        return Ok(Vec::new());
    }
    let sdk = Command::new("/usr/bin/xcrun")
        .args(["--show-sdk-path", "--sdk", "macosx"])
        .output()
        .context("xcrun")?;
    let sdk = String::from_utf8_lossy(&sdk.stdout).trim().to_owned();
    if sdk.is_empty() {
        bail!("no macOS SDK (xcrun --show-sdk-path --sdk macosx)");
    }
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    Ok(vec![
        "-target".into(),
        format!("{arch}-apple-ios{CATALYST_IOS}-macabi"),
        "-sdk".into(),
        sdk.clone(),
        "-F".into(),
        format!("{sdk}/System/iOSSupport/System/Library/Frameworks"),
        "-L".into(),
        format!("{sdk}/System/iOSSupport/usr/lib"),
    ])
}

/// A fixture's Info.plist keys past the name and id: an AppKit app names its principal class; a
/// UIKit app declares its scenes and must not name one (it crashes at launch if it does).
pub fn plist_extra(name: &str) -> &'static str {
    if name == CATALYST {
        "<key>UIApplicationSceneManifest</key><dict><key>UIApplicationSupportsMultipleScenes</key><false/></dict>\n<key>CFBundleVersion</key><string>1</string>\n"
    } else {
        "<key>NSPrincipalClass</key><string>NSApplication</string>\n"
    }
}

fn newer(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => matches!((a.modified(), b.modified()), (Ok(x), Ok(y)) if x > y),
        _ => false,
    }
}

/// The pinned Electron app, installed on first use.
fn electron_runtime() -> Result<PathBuf> {
    let dir = suite_dir().join("electron-runtime");
    let app = dir.join("node_modules/electron/dist/Electron.app");
    let version = dir.join("node_modules/electron/dist/version");
    let have = std::fs::read_to_string(&version).unwrap_or_default();
    if app.exists() && have.trim() == ELECTRON_VERSION {
        return Ok(app);
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("package.json"), "{\"private\":true}\n")?;
    let st = Command::new("npm")
        .args(["install", "--no-audit", "--no-fund", "--save-exact"])
        .arg(format!("electron@{ELECTRON_VERSION}"))
        .current_dir(&dir)
        .stdout(Stdio::null())
        .status()
        .context("npm is needed to install the Electron fixture's runtime")?;
    if !st.success() {
        bail!("npm couldn't install electron@{ELECTRON_VERSION}");
    }
    // Electron's package fetches its binary (checked against its checksums) when first run.
    let st = Command::new("node")
        .arg("node_modules/electron/install.js")
        .current_dir(&dir)
        .stdout(Stdio::null())
        .status()
        .context("node")?;
    if !st.success() || !app.exists() {
        bail!("Electron {ELECTRON_VERSION}'s binary didn't install");
    }
    Ok(app)
}

/// The Electron fixture's app folder: its files, and the addon built when its source changed.
fn electron_app_dir() -> Result<PathBuf> {
    let src = fixture_src(ELECTRON);
    let dir = suite_dir().join(ELECTRON);
    std::fs::create_dir_all(&dir)?;
    for f in ["main.js", "preload.js", "index.html", "package.json"] {
        std::fs::copy(src.join(f), dir.join(f))?;
    }
    let addon = dir.join("order_back.node");
    let m = src.join("order_back.m");
    if !addon.exists() || newer(&m, &addon) {
        let st = Command::new("/usr/bin/clang")
            .args([
                "-bundle",
                "-undefined",
                "dynamic_lookup",
                "-fobjc-arc",
                "-framework",
                "AppKit",
            ])
            .arg(&m)
            .arg("-o")
            .arg(&addon)
            .status()
            .context("clang (the Xcode command line tools) builds the Electron fixture's addon")?;
        if !st.success() {
            bail!("the Electron fixture's addon didn't build");
        }
    }
    Ok(dir)
}

/// Opens the Electron fixture in the background and returns its main process's pid.
fn launch_electron(log: &Path, data: &Path) -> Result<i32> {
    let app = electron_runtime()?;
    let dir = electron_app_dir()?;
    let pid_file = std::env::temp_dir().join(format!(
        "brigadier-fixture-{}-{}.pid",
        std::process::id(),
        Instant::now().elapsed().as_nanos() ^ u128::from(std::process::id())
    ));
    let _ = std::fs::remove_file(&pid_file);
    let st = Command::new("/usr/bin/open")
        .args(["-n", "-g", "--env"])
        .arg(format!("FIXTURE_PID_FILE={}", pid_file.display()))
        .arg(&app)
        .arg("--args")
        .arg(&dir)
        .arg(log)
        .arg(data)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .context("open")?;
    if !st.success() {
        bail!("open couldn't start the Electron fixture");
    }
    suite_run::wait_pid_file(&pid_file, "electron-pad")
}

/// Sets a hard-surface task up: a fresh instance of its fixture with a log, a scratch folder
/// and (for Electron) a scratch user-data folder in `dir`.
pub fn setup(desktop: &mut MacDesktop, task: &Task, dir: &Path, prep: &mut Prepared) -> Result<()> {
    let Setup::Quirk { fixture, window } = task.setup else {
        bail!("{} isn't a hard-surface task", task.id);
    };
    let log = dir.join("fixture-log.jsonl");
    prep.pid = match fixture {
        ELECTRON => launch_electron(&log, &dir.join("electron-data"))?,
        QUIRK => {
            let save = dir.join("save");
            std::fs::create_dir_all(&save)?;
            prep.files
                .insert(SAVED.to_owned(), save.join(SAVED).display().to_string());
            suite_run::launch_fixture(QUIRK, &[log.as_os_str(), save.as_os_str()])?
        }
        CATALYST => suite_run::launch_fixture(CATALYST, &[log.as_os_str()])?,
        other => bail!("no fixture {other}"),
    };
    prep.pid_start_us = crate::macos::process_start_us(prep.pid).unwrap_or(0);
    prep.exe = suite_run::executable(prep.pid).unwrap_or_default();
    prep.log = Some(log.display().to_string());
    // Electron builds its page's tree about 2 s after the first client asks.
    prep.window = suite_run::wait_window(desktop, prep, window, 20)?;
    prep.window_title = window.to_owned();
    if fixture == QUIRK && window == "Minimised Pad" {
        wait_minimised(desktop, prep)?;
    }
    if fixture == ELECTRON {
        wait_page(desktop, prep)?;
    }
    Ok(())
}

fn wait_minimised(desktop: &mut MacDesktop, prep: &Prepared) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if desktop.window(prep.window).is_ok_and(|w| w.minimized) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    suite_run::teardown(prep);
    bail!("the Minimised Pad window didn't minimise")
}

/// Waits until the Electron page's tree is there, so a trial's clock starts on a ready app.
fn wait_page(desktop: &mut MacDesktop, prep: &Prepared) -> Result<()> {
    let w = desktop.window(prep.window).map_err(|e| anyhow!("{e}"))?;
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if desktop.structure(&w) == crate::desktop::Structure::Ready {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    suite_run::teardown(prep);
    bail!("the Electron page's accessibility tree didn't appear")
}

/// The scripted solution of a hard-surface task; `None` for any other task.
pub(crate) fn solve(s: &mut Script, task: &Task) -> Option<Result<()>> {
    let r = match task.id {
        "electron-signup" => (|| {
            let r = s.observe(None, Screenshot::Never)?;
            let name = suite_run::ref_on(&r.text, "textfield \"Full name\"")?;
            let sub = suite_run::ref_on(&r.text, "checkbox \"Subscribe\"")?;
            let submit = suite_run::ref_on(&r.text, "button \"Submit\"")?;
            s.act(vec![
                set(&name, "Ada Lovelace"),
                click(&sub),
                click(&submit),
            ])?;
            Ok(())
        })(),
        "swiftui-item" => (|| {
            let r = s.observe(None, Screenshot::Never)?;
            let title = suite_run::ref_on(&r.text, "textfield \"Title\"")?;
            let starred = suite_run::ref_on(&r.text, "checkbox \"Starred\"")?;
            let large = suite_run::ref_on(&r.text, "radio \"Large\"")?;
            let save = suite_run::ref_on(&r.text, "button \"Save Item\"")?;
            s.act(vec![
                set(&title, "Quarterly report"),
                click(&starred),
                click(&large),
                click(&save),
            ])?;
            Ok(())
        })(),
        "catalyst-order" => (|| {
            let r = s.observe(None, Screenshot::Never)?;
            let up = suite_run::ref_on(&r.text, "button \"Quantity, Increment\"")?;
            let note = suite_run::ref_on(&r.text, "textfield \"Note\"")?;
            let place = suite_run::ref_on(&r.text, "button \"Place Order\"")?;
            s.act(vec![
                click(&up),
                click(&up),
                click(&up),
                set(&note, "gift wrap"),
                click(&place),
            ])?;
            Ok(())
        })(),
        "save-panel" => (|| {
            let export = s.find("button \"Export…\"")?;
            s.act(vec![click(&export)])?;
            // The panel is a sheet on the pad's window, in its tree.
            let r = s.observe(None, Screenshot::Never)?;
            let name = suite_run::ref_on(&r.text, "textfield value=\"Untitled.txt\"")?;
            let save = suite_run::ref_on(&r.text, "button \"Save\"")?;
            s.act(vec![set(&name, SAVED), click(&save)])?;
            Ok(())
        })(),
        "minimised-code" => (|| {
            let r = s.observe(None, Screenshot::Never)?;
            let code = suite_run::ref_on(&r.text, "textfield \"Code\"")?;
            let apply = suite_run::ref_on(&r.text, "button \"Apply\"")?;
            s.act(vec![set(&code, "4711"), click(&apply)])?;
            Ok(())
        })(),
        _ => return None,
    };
    Some(r)
}
