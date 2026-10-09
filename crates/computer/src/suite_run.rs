//! The suite's macOS side (§7, Phase 4): sets a task's target up in the background, runs the
//! scripted (no-model) solution through the engine, reads the end state back and watches the
//! user's focus independently of the engine's own checks.
//!
//! The scripted solutions debug the checkers and give each task its reference: the fewest `act`
//! batches a script needs (E1's denominator).

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};

use crate::action::{ActRequest, Action, Expect, ObserveRequest, Reply, Screenshot, Target};
use crate::desktop::Desktop;
use crate::engine::Engine;
use crate::geom::{Point, Provider};
use crate::macos::MacDesktop;
use crate::suite::{self, BOARDS, MARKERS, Prepared, Setup, Task, Verdict};
use crate::tree;

const WORKER: &str = "suite";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A fixture app (`fixtures/<name>/main.swift`), built once per source change into a minimal
/// bundle, `target/computer-suite/<name>.app`, so LaunchServices can open it in the background.
pub fn fixture_app(name: &str) -> Result<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("fixtures").join(name).join("main.swift");
    let app = root
        .join("../../target/computer-suite")
        .join(format!("{name}.app"));
    let macos = app.join("Contents/MacOS");
    std::fs::create_dir_all(&macos)?;
    let bin = macos.join(name);
    let stale = match (std::fs::metadata(&bin), std::fs::metadata(&src)) {
        (Ok(b), Ok(s)) => b.modified()? < s.modified()?,
        _ => true,
    };
    if stale {
        let st = Command::new("swiftc")
            .arg("-O")
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .status()
            .context("swiftc (the Xcode command line tools) is needed to build the fixtures")?;
        if !st.success() {
            bail!("the {name} fixture didn't build");
        }
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>{name}</string>
<key>CFBundleIdentifier</key><string>dev.brigadier.fixture.{name}</string>
<key>CFBundleName</key><string>{name}</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
"#
            ),
        )?;
        let _ = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app)
            .stderr(Stdio::null())
            .status();
    }
    Ok(app)
}

/// Opens a fixture through LaunchServices in the background (`open -n -g`, a new instance that
/// isn't brought forward) and returns its pid. Started straight from the terminal the user is
/// in, a new app takes the front on launch, and the system switches to the Space that shows it.
/// Launched by LaunchServices, it outlives this command and nothing else signals it.
pub fn launch_fixture(name: &str, args: &[&std::ffi::OsStr]) -> Result<i32> {
    let app = fixture_app(name)?;
    let pid_file = std::env::temp_dir().join(format!(
        "brigadier-fixture-{}-{}.pid",
        std::process::id(),
        now_ms()
    ));
    let _ = std::fs::remove_file(&pid_file);
    let st = Command::new("/usr/bin/open")
        .args(["-n", "-g", "--env"])
        .arg(format!("FIXTURE_PID_FILE={}", pid_file.display()))
        .arg(&app)
        .arg("--args")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .context("open")?;
    if !st.success() {
        bail!("open couldn't start the {name} fixture");
    }
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(pid) = std::fs::read_to_string(&pid_file)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
        {
            let _ = std::fs::remove_file(&pid_file);
            return Ok(pid);
        }
        if Instant::now() > end {
            bail!("the {name} fixture didn't start");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The app's window titled `title`; the app is ended when it doesn't show one in time.
fn wait_window(desktop: &mut MacDesktop, prep: &Prepared, title: &str, secs: u64) -> Result<u32> {
    let pid = prep.pid;
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        // Its accessibility tree readable: an app registers the window with accessibility a
        // little after the window server shows it. It may be on another Space than the user's.
        if let Some(w) = desktop
            .windows(pid)
            .unwrap_or_default()
            .into_iter()
            .find(|w| w.title == title)
            && desktop.tree(&w, false).is_ok_and(|t| !t.is_empty())
        {
            return Ok(w.id);
        }
        if Instant::now() > end {
            teardown(prep);
            bail!("no window {title:?} from pid {pid}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Sets `task` up in `dir` (which it creates) and writes `dir/setup.json`. Every process it
/// starts is new and the suite's own: a fresh fixture, or a new TextEdit instance that restores
/// nothing.
pub fn setup(desktop: &mut MacDesktop, task: &Task, dir: &Path, seed: u64) -> Result<Prepared> {
    std::fs::create_dir_all(dir)?;
    let dir = dir.canonicalize()?;
    let mut prep = Prepared {
        task: task.id.to_owned(),
        ..Default::default()
    };
    match task.setup {
        Setup::Fixture | Setup::Grounding { .. } => {
            let log = dir.join("fixture-log.jsonl");
            let mut args: Vec<std::ffi::OsString> = vec![log.clone().into()];
            let title = match task.setup {
                Setup::Grounding { size } => {
                    for a in [
                        "--grounding".to_owned(),
                        size.to_string(),
                        "--seed".to_owned(),
                        seed.to_string(),
                        "--boards".to_owned(),
                        BOARDS.to_string(),
                    ] {
                        args.push(a.into());
                    }
                    format!("Grounding {size} pt")
                }
                _ => "Target Range".to_owned(),
            };
            let args: Vec<&std::ffi::OsStr> = args.iter().map(|a| a.as_os_str()).collect();
            prep.pid = launch_fixture("target-range", &args)?;
            prep.pid_start_us = crate::macos::process_start_us(prep.pid).unwrap_or(0);
            prep.window = wait_window(desktop, &prep, &title, 15)?;
            prep.window_title = title;
            prep.log = Some(log.display().to_string());
        }
        Setup::Document { file: (name, text) } => {
            let fdir = dir.join("files");
            std::fs::create_dir_all(&fdir)?;
            let p = fdir.join(name);
            std::fs::write(&p, text)?;
            prep.files.insert(name.to_owned(), p.display().to_string());
            prep.pid = launch_fixture("scratch-pad", &[p.as_os_str()])?;
            prep.pid_start_us = crate::macos::process_start_us(prep.pid).unwrap_or(0);
            prep.window = wait_window(desktop, &prep, name, 15)?;
            prep.window_title = name.to_owned();
        }
        Setup::DevApp => bail!("the runner sets the dev build up (tools/computer-suite)"),
    }
    // The app's own window notices settle first.
    std::thread::sleep(Duration::from_millis(400));
    prep.ready_ms = now_ms();
    std::fs::write(dir.join("setup.json"), serde_json::to_vec_pretty(&prep)?)?;
    Ok(prep)
}

/// Ends what `setup` started, by its own pid, while that pid is still the fixture it started.
pub fn teardown(prep: &Prepared) {
    if is_ours(prep) {
        let _ = Command::new("/bin/kill")
            .args(["-9", &prep.pid.to_string()])
            .stderr(Stdio::null())
            .status();
    }
}

/// Whether `prep.pid` is still the fixture `setup` started: the same start time and a fixture's
/// binary. After the fixture ends, its pid may name another process, which a signal would hit.
fn is_ours(prep: &Prepared) -> bool {
    if prep.pid <= 0
        || prep.pid_start_us == 0
        || crate::macos::process_start_us(prep.pid) != Some(prep.pid_start_us)
    {
        return false;
    }
    let comm = Command::new("/bin/ps")
        .args(["-o", "comm=", "-p", &prep.pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    comm.ends_with("/target-range") || comm.ends_with("/scratch-pad")
}

/// The scratch files' contents as they are on disk now.
pub fn read_files(prep: &Prepared) -> BTreeMap<String, String> {
    prep.files
        .iter()
        .filter_map(|(n, p)| Some((n.clone(), std::fs::read_to_string(p).ok()?)))
        .collect()
}

/// Asks a live fixture for its state snapshot and waits briefly for it in the log. Only the
/// fixture itself is signalled: after teardown its pid may belong to another process, which
/// SIGUSR1 would end.
fn snapshot(prep: &Prepared, log: &Path) {
    if prep.task.starts_with("Grounding") || prep.log.is_none() || !is_ours(prep) {
        return;
    }
    let before = suite::read_events(log).map(|e| e.len()).unwrap_or(0);
    let sent = Command::new("/bin/kill")
        .args(["-USR1", &prep.pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !sent {
        return;
    }
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        let events = suite::read_events(log).unwrap_or_default();
        if events.len() > before && suite::snapshot(&events[before..]).is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Checks a trial set up in `dir`, with the broker's records and the worker's report.
/// `live`: the fixture still runs and is asked for its state; offline, the log's last snapshot
/// stands (a trial checked again after teardown).
pub fn check_dir(
    dir: &Path,
    records: Option<&Path>,
    report: Option<&Path>,
    live: bool,
) -> Result<Verdict> {
    let prep: Prepared = serde_json::from_slice(&std::fs::read(dir.join("setup.json"))?)?;
    let events = match &prep.log {
        Some(l) => {
            if live {
                snapshot(&prep, Path::new(l));
            }
            suite::read_events(Path::new(l))?
        }
        None => Vec::new(),
    };
    let records = match records {
        Some(p) => suite::parse_records(&std::fs::read_to_string(p)?)?,
        None => Vec::new(),
    };
    let report = report.map(std::fs::read_to_string).transpose()?;
    Ok(suite::check(
        &prep,
        &events,
        &records,
        &read_files(&prep),
        report.as_deref(),
    ))
}

/// The line on an observation that contains `needle`, and its ref.
fn ref_on(text: &str, needle: &str) -> Result<String> {
    text.lines()
        .find(|l| l.contains(needle))
        .and_then(|l| l.split_whitespace().find(|w| tree::parse_ref(w).is_some()))
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("no element {needle:?} in:\n{text}"))
}

struct Script<'a> {
    engine: &'a mut Engine<MacDesktop>,
    window: u32,
    batches: u32,
    calls: u32,
    transcript: String,
}

impl Script<'_> {
    fn observe(&mut self, find: Option<&str>, shot: Screenshot) -> Result<Reply> {
        self.calls += 1;
        let r = self
            .engine
            .observe(
                WORKER,
                &ObserveRequest {
                    window: self.window,
                    screenshot: shot,
                    since: None,
                    full: true,
                    element: None,
                    find: find.map(str::to_owned),
                    value_page: None,
                },
            )
            .map_err(|e| anyhow!("observe: {e}"))?;
        self.transcript.push_str(&r.text);
        Ok(r)
    }

    fn find(&mut self, needle: &str) -> Result<String> {
        let r = self.observe(Some(needle), Screenshot::Never)?;
        ref_on(&r.text, needle)
    }

    fn act(&mut self, actions: Vec<Action>) -> Result<Reply> {
        self.batches += 1;
        self.calls += 1;
        let r = self
            .engine
            .act(
                WORKER,
                &ActRequest {
                    window: self.window,
                    actions,
                    screenshot: Screenshot::Never,
                },
            )
            .map_err(|e| anyhow!("act: {e}"))?;
        self.transcript.push_str(&r.text);
        Ok(r)
    }

    /// A point given in window points, as a pixel of a fresh screenshot.
    fn pixel(&mut self, at: &[Point]) -> Result<Vec<Target>> {
        let r = self.observe(None, Screenshot::Always)?;
        let img = r.image.context("a screenshot")?;
        let t = self
            .engine
            .image_transform(&img.id)
            .context("the image's transform")?;
        Ok(at
            .iter()
            .map(|p| {
                let q = t.to_image(*p);
                Target {
                    image: Some(img.id.clone()),
                    x: Some(q.x),
                    y: Some(q.y),
                    r#ref: None,
                }
            })
            .collect())
    }
}

fn click(r: &str) -> Action {
    Action::Click {
        target: Target {
            r#ref: Some(r.to_owned()),
            ..Default::default()
        },
        button: Default::default(),
        count: 1,
        modifiers: vec![],
        expect: None,
    }
}

fn click_at(t: Target) -> Action {
    Action::Click {
        target: t,
        button: Default::default(),
        count: 1,
        modifiers: vec![],
        expect: None,
    }
}

fn set(r: &str, text: &str) -> Action {
    Action::SetValue {
        r#ref: r.to_owned(),
        text: text.to_owned(),
        expect: Some(Expect::ValueEquals {
            r#ref: r.to_owned(),
            text: text.to_owned(),
        }),
    }
}

fn key(k: &str) -> Action {
    Action::Key {
        key: k.to_owned(),
        repeat: 1,
        expect: None,
    }
}

/// A fixture dot's centre in window points (fixture `main.swift`: the canvas at 580,10 of a
/// 600 pt high content view).
fn dot(frame_h: f64, cx: f64, cy: f64) -> Point {
    Point::new(580.0 + cx, (frame_h - 600.0) + 10.0 + cy)
}

/// Runs `task`'s scripted solution on its prepared target.
fn solve(s: &mut Script, task: &Task, prep: &Prepared) -> Result<()> {
    let frame_h = s
        .engine
        .desktop
        .window(prep.window)
        .map_err(|e| anyhow!("{e}"))?
        .frame
        .h;
    match task.id {
        "slider" => {
            let r = s.find("slider \"Level\"")?;
            s.act(vec![set(&r, "37")])?;
        }
        "check-8" => {
            let r = s.find("checkbox \"Check 8 pt\"")?;
            s.act(vec![click(&r)])?;
        }
        "menu" => {
            s.act(vec![Action::Menu {
                path: ["Targets", "Level 1", "Level 2", "Pick Me 3"]
                    .map(str::to_owned)
                    .to_vec(),
                expect: None,
            }])?;
        }
        "row-173" | "last-row" => {
            let row = if task.id == "row-173" { "173" } else { "199" };
            let r = s.find(&format!("row \"Row {row}\""))?;
            s.act(vec![click(&r)])?;
        }
        "red-dot" => {
            let t = s.pixel(&[dot(frame_h, 60.0, 30.0)])?;
            s.act(t.into_iter().map(click_at).collect())?;
        }
        "two-dots" => {
            let t = s.pixel(&[dot(frame_h, 60.0, 210.0), dot(frame_h, 60.0, 300.0)])?;
            s.act(t.into_iter().map(click_at).collect())?;
        }
        "name" => {
            let r = s.find("textfield \"Name\"")?;
            s.act(vec![set(&r, "Ada Lovelace")])?;
        }
        "stepper" => {
            // A stepper takes no value; seven presses of its increment arrow from 5.
            let r = s.find("button \"increment\"")?;
            s.act(vec![click(&r); 7])?;
        }
        "popup" => {
            let r = s.find("popup \"Letter\"")?;
            s.act(vec![set(&r, "Gamma")])?;
        }
        "sheet" => {
            let r = s.find("button \"Open Sheet\"")?;
            s.act(vec![click(&r)])?;
            let r = s.find("button \"Close Sheet\"")?;
            s.act(vec![click(&r)])?;
        }
        "tab" => {
            let r = s.find("radio \"Second\"")?;
            s.act(vec![click(&r)])?;
        }
        "press-3" => {
            let r = s.find("button \"Button 12 pt\"")?;
            s.act(vec![click(&r), click(&r), click(&r)])?;
        }
        "form" => {
            let o = s.observe(None, Screenshot::Never)?;
            let name = ref_on(&o.text, "textfield \"Name\"")?;
            let notes = ref_on(&o.text, "textfield \"Notes\"")?;
            let check = ref_on(&o.text, "checkbox \"Check 16 pt\"")?;
            s.act(vec![
                set(&name, "Grace"),
                set(&notes, "compiler"),
                click(&check),
            ])?;
        }
        "password" => {
            // The engine must refuse both ways in; the checker reads the records.
            let r = s.find("secure-field \"Password\"")?;
            let typed = s.act(vec![Action::Type {
                text: "hunter2".into(),
                r#ref: Some(r.clone()),
                expect: None,
            }])?;
            let set_value = s.act(vec![Action::SetValue {
                r#ref: r,
                text: "hunter2".into(),
                expect: None,
            }])?;
            for (how, reply) in [("type", &typed), ("set_value", &set_value)] {
                if reply
                    .results
                    .first()
                    .and_then(|x| x.error.as_ref())
                    .is_none()
                {
                    bail!("the engine let {how} into the secure field");
                }
            }
        }
        "replace-text" => {
            let r = s.find("text-area")?;
            s.act(vec![set(&r, "Final copy"), key("cmd+s")])?;
        }
        "find-replace" => {
            // The app's own find bar: typed text could be changed by the system's substitutions.
            s.act(vec![Action::Menu {
                path: ["Edit", "Find", "Find and Replace…"]
                    .map(str::to_owned)
                    .to_vec(),
                expect: Some(Expect::Appears {
                    find: "button \"All\"".into(),
                }),
            }])?;
            let o = s.observe(None, Screenshot::Never)?;
            let fields: Vec<String> = o
                .text
                .lines()
                .filter(|l| !l.starts_with("focus:"))
                .filter(|l| l.contains("search-field") || l.contains("textfield"))
                .filter_map(|l| l.split_whitespace().find(|w| tree::parse_ref(w).is_some()))
                .map(str::to_owned)
                .collect();
            let [find, replace] = fields.as_slice() else {
                bail!("the find bar's two fields, in:\n{}", o.text);
            };
            let all = ref_on(&o.text, "button \"All\"")?;
            s.act(vec![
                set(find, "foo"),
                set(replace, "qux"),
                click(&all),
                key("cmd+s"),
            ])?;
        }
        "append-line" => {
            let r = s.find("text-area")?;
            let len = "line one\n".chars().count();
            s.act(vec![
                Action::Select {
                    r#ref: r.clone(),
                    start: len,
                    length: 0,
                    expect: None,
                },
                Action::Type {
                    text: "line two\n".into(),
                    r#ref: None,
                    expect: None,
                },
                key("cmd+s"),
            ])?;
        }
        _ if matches!(task.setup, Setup::Grounding { .. }) => {
            let log = PathBuf::from(prep.log.as_deref().context("the boards' log")?);
            for board in 1..=BOARDS {
                let events = suite::read_events(&log)?;
                let layout = events
                    .iter()
                    .rev()
                    .find(|e| e.id == "board" && e.ev == "layout")
                    .and_then(|e| e.v.clone())
                    .context("a dealt board")?;
                let centres: BTreeMap<&str, (f64, f64)> = layout
                    .split(' ')
                    .skip(1)
                    .filter_map(|p| {
                        let (id, xy) = p.split_once(':')?;
                        let (x, y) = xy.split_once(',')?;
                        Some((id, (x.parse().ok()?, y.parse().ok()?)))
                    })
                    .collect();
                // The canvas sits at 10,50 of the content view.
                let title_bar = frame_h - 520.0;
                let pts: Vec<Point> = (1..=MARKERS)
                    .map(|i| {
                        let (x, y) = centres[format!("m{i}").as_str()];
                        Point::new(10.0 + x, title_bar + 50.0 + y)
                    })
                    .collect();
                let targets = s.pixel(&pts)?;
                let next = ref_on(&s.transcript, "button \"Next board\"")
                    .or_else(|_| s.find("button \"Next board\""))?;
                let mut acts: Vec<Action> = targets.into_iter().map(click_at).collect();
                acts.push(click(&next));
                s.act(acts)?;
                let _ = board;
            }
        }
        other => bail!("no scripted solution for {other}"),
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct ScriptedRun {
    pub task: String,
    pub verdict: Verdict,
    /// The fewest `act` batches the script needs (E1's denominator).
    pub reference_batches: u32,
    /// Every tool call the script made (observe, act).
    pub tool_calls: u32,
    pub wall_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grounding: Option<suite::Grounding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Runs the scripted solution of every task named (all with a scripted setup when none are)
/// and checks each one with the same checker the model runs get.
pub fn scripted(desktop: MacDesktop, out: &Path, only: &[String]) -> Result<bool> {
    std::fs::create_dir_all(out)?;
    let out = out.canonicalize()?;
    let mut engine = Engine::new(desktop, crate::harness::dev_block_list(), Provider::Claude);
    let tasks: Vec<&Task> = suite::TASKS
        .iter()
        .chain(suite::GROUNDING.iter())
        .filter(|t| !matches!(t.setup, Setup::DevApp))
        .filter(|t| {
            only.is_empty()
                || only
                    .iter()
                    .any(|o| suite::task(o).is_some_and(|x| x.id == t.id))
        })
        .collect();
    let mut runs = Vec::new();
    for task in tasks {
        let dir = out.join(task.id.replace(' ', "-"));
        let _ = std::fs::remove_dir_all(&dir);
        let t0 = Instant::now();
        let prep = setup(&mut engine.desktop, task, &dir, 1)?;
        engine.desktop.watch(prep.pid);
        let mut s = Script {
            engine: &mut engine,
            window: prep.window,
            batches: 0,
            calls: 0,
            transcript: String::new(),
        };
        let solved = solve(&mut s, task, &prep);
        let (batches, calls, transcript) = (s.batches, s.calls, std::mem::take(&mut s.transcript));
        // The app writes its last events (and TextEdit its file) before the end state is read.
        std::thread::sleep(Duration::from_millis(600));
        let mut records = String::new();
        for r in engine.records.drain(..) {
            records.push_str(&serde_json::to_string(&r)?);
            records.push('\n');
        }
        std::fs::write(dir.join("records.jsonl"), &records)?;
        std::fs::write(dir.join("transcript.txt"), &transcript)?;
        let verdict = check_dir(&dir, Some(&dir.join("records.jsonl")), None, true)?;
        let grounding = match task.setup {
            Setup::Grounding { size } => Some(suite::score_grounding(
                size,
                &suite::read_events(Path::new(prep.log.as_deref().unwrap_or_default()))?,
            )),
            _ => None,
        };
        teardown(&prep);
        engine.forget_worker(WORKER);
        let run = ScriptedRun {
            task: task.id.to_owned(),
            verdict,
            reference_batches: batches,
            tool_calls: calls,
            wall_ms: t0.elapsed().as_millis() as u64,
            grounding,
            error: solved.err().map(|e| format!("{e:#}")),
        };
        println!(
            "{:<16} {:<4} batches {:>2} calls {:>2} {:>6} ms{}{}",
            run.task,
            if run.verdict.pass && run.error.is_none() {
                "pass"
            } else {
                "FAIL"
            },
            run.reference_batches,
            run.tool_calls,
            run.wall_ms,
            run.verdict
                .outcome
                .as_deref()
                .map(|o| format!(" ({o})"))
                .unwrap_or_default(),
            if run.verdict.notes.is_empty() && run.error.is_none() {
                String::new()
            } else {
                format!(
                    "  {}{}",
                    run.verdict.notes.join("; "),
                    run.error
                        .as_deref()
                        .map(|e| format!(" error: {e}"))
                        .unwrap_or_default()
                )
            }
        );
        runs.push(run);
    }
    let ok = runs.iter().all(|r| r.verdict.pass && r.error.is_none());
    std::fs::write(out.join("scripted.json"), serde_json::to_vec_pretty(&runs)?)?;
    println!("results in {}", out.join("scripted.json").display());
    Ok(ok)
}

/// The independent focus monitor (F1): samples the user's side of the desktop every 50 ms and
/// writes each change, with its time, to `out` until its standard input closes.
pub fn watch(mut desktop: MacDesktop, out: &Path) -> Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::File::create(out)?;
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 64];
        let mut stdin = std::io::stdin();
        while matches!(stdin.read(&mut buf), Ok(n) if n > 0) {}
        let _ = tx.send(());
    });
    let mut last = desktop.user_focus();
    writeln!(f, "{}", json!({"at_ms": now_ms(), "start": last}))?;
    loop {
        if rx.recv_timeout(Duration::from_millis(50)).is_ok() {
            break;
        }
        let now = desktop.user_focus();
        if now != last {
            let mut what: Vec<&str> = Vec::new();
            if now.frontmost_pid != last.frontmost_pid {
                what.push("frontmost_app");
            }
            if now.frontmost_window_id != last.frontmost_window_id
                || now.frontmost_window != last.frontmost_window
            {
                what.push("key_window");
            }
            if now.cursor != last.cursor {
                what.push("cursor");
            }
            if now.server_front != last.server_front {
                what.push("server_front");
            }
            let entry: Value = json!({"at_ms": now_ms(), "changed": what, "from": last, "to": now});
            writeln!(f, "{entry}")?;
            f.flush()?;
            last = now;
        }
    }
    writeln!(f, "{}", json!({"at_ms": now_ms(), "end": last}))?;
    Ok(())
}
