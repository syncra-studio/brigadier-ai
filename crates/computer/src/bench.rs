//! The benchmark (§1, §7): launches the fixture in the background, drives every target through
//! the paths it supports, reads the fixture's own log as ground truth and checks the gates.
//!
//! Timings, per action:
//! - dispatch: the request's pre-action checks plus delivery (`checks_ms + dispatch_ms`);
//! - effect: the fixture's log line for the expected event, minus the request's wall-clock start;
//! - quiet: the engine's settle time (no accessibility notification for 50 ms).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::Value;

use crate::action::{
    ActRequest, Action, ActionResult, ImageOut, ObserveRequest, Reply, Rung, Screenshot, Status,
};
use crate::cursor::Cursor;
use crate::desktop::{Desktop, UserFocus, WindowInfo};
use crate::engine::Engine;
use crate::error::ErrorCode;
use crate::geom::{Point, Provider};
use crate::macos::MacDesktop;
use crate::tree;

/// Where the fixture's dot canvas sits in its window's content (fixture `main.swift`).
const CANVAS_ORIGIN: (f64, f64) = (580.0, 10.0);
/// The fixture's content height; the window's frame adds the title bar.
const CONTENT_HEIGHT: f64 = 600.0;
/// The dots: id, size, centre in canvas points (fixture `main.swift`).
const DOTS: [(&str, f64, f64, f64); 8] = [
    ("dot-8", 8.0, 60.0, 30.0),
    ("dot-8-b", 8.0, 200.0, 70.0),
    ("dot-12", 12.0, 60.0, 120.0),
    ("dot-12-b", 12.0, 200.0, 160.0),
    ("dot-16", 16.0, 60.0, 210.0),
    ("dot-16-b", 16.0, 200.0, 250.0),
    ("dot-24", 24.0, 60.0, 300.0),
    ("dot-24-b", 24.0, 200.0, 340.0),
];
const SIZES: [u32; 4] = [8, 12, 16, 24];

fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

#[derive(Debug, Clone, Serialize)]
struct Event {
    t: f64,
    id: String,
    ev: String,
    x: Option<f64>,
    y: Option<f64>,
    v: Option<String>,
}

/// The fixture's log, read from where the last read stopped.
struct LogTail {
    path: PathBuf,
    offset: u64,
}

impl LogTail {
    fn read_new(&mut self) -> Result<Vec<Event>> {
        let mut f = std::fs::File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut r = BufReader::new(f);
        let mut out = Vec::new();
        let mut line = String::new();
        loop {
            line.clear();
            let n = r.read_line(&mut line)?;
            // A line still being written is read again next time.
            if n == 0 || !line.ends_with('\n') {
                break;
            }
            self.offset += n as u64;
            let v: Value = serde_json::from_str(line.trim())?;
            out.push(Event {
                t: v["t"].as_f64().unwrap_or(0.0),
                id: v["id"].as_str().unwrap_or("").to_owned(),
                ev: v["ev"].as_str().unwrap_or("").to_owned(),
                x: v["x"].as_f64(),
                y: v["y"].as_f64(),
                v: v["v"].as_str().map(str::to_owned),
            });
        }
        Ok(out)
    }

    /// New events until one matches, or `wait` passes.
    fn until(&mut self, wait: Duration, found: impl Fn(&Event) -> bool) -> Result<Vec<Event>> {
        let end = Instant::now() + wait;
        let mut all = Vec::new();
        loop {
            all.extend(self.read_new()?);
            if all.iter().any(&found) || Instant::now() >= end {
                return Ok(all);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Samples of one timing.
#[derive(Debug, Default, Clone, Serialize)]
struct Samples(Vec<f64>);

impl Samples {
    fn pct(&self, p: f64) -> Option<f64> {
        if self.0.is_empty() {
            return None;
        }
        let mut v = self.0.clone();
        v.sort_by(f64::total_cmp);
        let i = ((p / 100.0) * (v.len() - 1) as f64).round() as usize;
        Some(v[i.min(v.len() - 1)])
    }
}

#[derive(Debug, Default, Serialize)]
struct Op {
    /// The part of dispatch spent on the pre-action checks.
    checks: Samples,
    dispatch: Samples,
    effect: Samples,
    quiet: Samples,
    ok: u32,
    tried: u32,
    /// Why samples failed, by reason.
    misses: BTreeMap<String, u32>,
}

impl Op {
    fn miss(&mut self, why: impl Into<String>) {
        *self.misses.entry(why.into()).or_default() += 1;
    }
}

#[derive(Debug, Serialize)]
struct Gate {
    id: &'static str,
    what: String,
    measured: String,
    target: String,
    pass: bool,
}

#[derive(Default, Serialize)]
struct Report {
    reps: u32,
    ops: BTreeMap<String, Op>,
    /// P2: distance between the point asked for and the point the fixture received, in points.
    mapping_error_pt: Samples,
    mapping_outside_target: u32,
    /// Wrong-target actions with an effect: (action, event).
    wrong_target: Vec<(String, Event)>,
    /// Focus changes seen around actions (F1).
    focus_changes: Vec<String>,
    /// Focus changes the fixture can't have caused, listed, not counted.
    outside_focus_changes: Vec<String>,
    /// Supported paths per target kind.
    coverage: BTreeMap<String, Vec<String>>,
    t1_text_tokens: usize,
    t2_image: String,
    gates: Vec<Gate>,
}

struct Bench {
    engine: Engine<MacDesktop>,
    log: LogTail,
    win: WindowInfo,
    mini: WindowInfo,
    report: Report,
    user: UserFocus,
    replay: Option<ReplayLog>,
}

/// The run's action log, written the way the daemon keeps it (one `ComputerAction` per action,
/// core's `work.rs`), so the app's timeline can replay a bench run.
struct ReplayLog {
    dir: PathBuf,
    actions: Vec<Value>,
    batches: u32,
}

impl ReplayLog {
    /// One `act`'s records; its trajectory image goes beside the log as `<batch>.png`.
    fn batch(
        &mut self,
        records: &[crate::record::ActionRecord],
        trajectory: Option<ImageOut>,
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        self.batches += 1;
        let batch = format!("bench-{:05}", self.batches);
        let image = match trajectory {
            Some(t) => {
                std::fs::write(self.dir.join(format!("{batch}.png")), &t.png)?;
                Some(batch.clone())
            }
            None => None,
        };
        for (i, r) in records.iter().enumerate() {
            self.actions.push(serde_json::json!({
                "batch": batch,
                "index": r.index,
                "atMs": r.at_ms,
                "kind": r.action.kind(),
                "app": r.app,
                "appWindow": r.window_title,
                "target": r.target,
                "pid": r.pid,
                "window": r.window,
                "status": wire_name(&r.status),
                "rung": r.rung.as_ref().and_then(wire_name),
                "effect": r.effect.as_ref().and_then(wire_name),
                "error": r.error.map(|c| c.as_str()),
                "detail": r.detail,
                "dispatchMs": r.timings.dispatch_ms,
                "record": serde_json::to_string(r)?,
                "image": if i == 0 { image.clone() } else { None },
            }));
        }
        Ok(())
    }

    fn write(&self) -> Result<PathBuf> {
        let path = self.dir.join("actions.json");
        std::fs::write(&path, serde_json::to_vec_pretty(&self.actions)?)?;
        Ok(path)
    }
}

/// An enum's name as it reads on the wire (`background_activated`).
fn wire_name(v: &impl Serialize) -> Option<String> {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
}

fn ms_since(t0: f64, t: f64) -> f64 {
    ((t - t0) * 100.0).round() / 100.0
}

/// The ref on the observation line that contains `needle`.
fn find_ref(text: &str, needle: &str) -> Result<String> {
    text.lines()
        .find(|l| l.contains(needle))
        .and_then(|l| l.split_whitespace().find(|w| tree::parse_ref(w).is_some()))
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("no element {needle:?} in the observation"))
}

impl Bench {
    fn observe(&mut self, window: u32, shot: Screenshot, full: bool) -> Result<Reply> {
        self.engine
            .observe(
                "bench",
                &ObserveRequest {
                    window,
                    screenshot: shot,
                    since: None,
                    full,
                    element: None,
                    find: None,
                    value_page: None,
                },
            )
            .map_err(|e| anyhow!("observe: {e}"))
    }

    /// Runs one action; returns its result and the wall-clock start of the request.
    fn act(&mut self, window: u32, action: Action) -> Result<(ActionResult, f64)> {
        let before = self.engine.desktop.user_focus();
        let t0 = epoch_ms();
        let reply = self
            .engine
            .act(
                "bench",
                &ActRequest {
                    window,
                    actions: vec![action],
                    screenshot: Screenshot::Never,
                },
            )
            .map_err(|e| anyhow!("act: {e}"))?;
        self.logged(reply.trajectory.clone())?;
        let what = serde_json::to_string(&reply.results.first().map(|r| &r.action))?;
        self.check_focus(&what, &before);
        let r = reply
            .results
            .into_iter()
            .next()
            .context("act returned no result")?;
        Ok((r, t0))
    }

    /// Hands the last `act`'s records to the replay log, when there is one, and clears them.
    fn logged(&mut self, trajectory: Option<ImageOut>) -> Result<()> {
        let records = std::mem::take(&mut self.engine.records);
        match &mut self.replay {
            Some(log) => log.batch(&records, trajectory),
            None => Ok(()),
        }
    }

    /// Compares the user's side of the desktop before and after `what` (F1). A change the
    /// fixture can't have caused, the frontmost app quitting, is listed apart and not counted.
    fn check_focus(&mut self, what: &str, before: &UserFocus) {
        let after = self.engine.desktop.user_focus();
        if after == *before {
            return;
        }
        let quit = after.frontmost_pid != before.frontmost_pid
            && after.frontmost_pid != self.win.pid
            && after.cursor == before.cursor
            && self.engine.desktop.app(before.frontmost_pid).is_err();
        let line = format!("{what}: {before:?} → {after:?}");
        if quit {
            self.report.outside_focus_changes.push(format!(
                "the frontmost app (pid {}) quit during {line}",
                before.frontmost_pid
            ));
        } else {
            self.report.focus_changes.push(line);
        }
    }

    /// Records one action against the event it should cause on control `target`. An event on
    /// any other control is a wrong-target action (P4); the fixture's own activation notices and
    /// the bare canvas (a miss with no effect) are not.
    fn score(
        &mut self,
        op: &str,
        r: &ActionResult,
        t0: f64,
        target: &str,
        expected: impl Fn(&Event) -> bool,
    ) -> Result<Option<Event>> {
        let events = self.log.until(Duration::from_millis(1_000), &expected)?;
        let label = serde_json::to_string(&r.action).unwrap_or_default();
        for e in &events {
            if e.id != target && e.id != "app" && e.id != "canvas" {
                self.report
                    .wrong_target
                    .push((format!("{op} {label}"), e.clone()));
            }
        }
        let hit = events.iter().find(|e| expected(e)).cloned();
        let o = self.report.ops.entry(op.to_owned()).or_default();
        o.tried += 1;
        if r.status != Status::Done {
            o.miss(
                r.error
                    .as_ref()
                    .map_or("failed".to_owned(), |e| e.code.as_str().to_owned()),
            );
            return Ok(None);
        }
        o.checks.0.push(r.timings.checks_ms);
        o.dispatch
            .0
            .push(r.timings.checks_ms + r.timings.dispatch_ms);
        o.quiet.0.push(r.timings.settle_ms);
        match &hit {
            Some(e) => {
                o.ok += 1;
                o.effect.0.push(ms_since(t0, e.t));
            }
            None => o.miss(match events.iter().find(|e| e.id == "canvas") {
                Some(_) => "landed on the canvas, not the target".to_owned(),
                None => "no effect in the fixture's log".to_owned(),
            }),
        }
        Ok(hit)
    }

    /// S1, S2: observations, structure only and with a screenshot.
    fn observations(&mut self, reps: u32) -> Result<()> {
        let id = self.win.id;
        for (op, shot) in [
            ("S1 observe", Screenshot::Never),
            ("S2 observe+shot", Screenshot::Always),
        ] {
            for _ in 0..reps {
                let t = Instant::now();
                let reply = self.observe(id, shot, true)?;
                let took = t.elapsed().as_secs_f64() * 1000.0;
                let o = self.report.ops.entry(op.to_owned()).or_default();
                o.tried += 1;
                o.ok += 1;
                o.dispatch.0.push(took);
                if shot == Screenshot::Always {
                    if let Some(img) = &reply.image {
                        self.report.t2_image = format!(
                            "{}x{} px, ≈{} Claude visual tokens, {} KB png",
                            img.width,
                            img.height,
                            img.tokens,
                            img.png.len() / 1024
                        );
                    }
                } else {
                    self.report.t1_text_tokens = tree::text_tokens(&reply.text);
                }
            }
        }
        Ok(())
    }

    /// S3 and P1: element presses on every sized target, a slider value, a pop-up pick.
    fn elements(&mut self, reps: u32) -> Result<()> {
        let id = self.win.id;
        let text = self.observe(id, Screenshot::Never, true)?.text;
        for size in SIZES {
            for (kind, label, ev) in [
                ("button", format!("\"Button {size} pt\""), "press"),
                ("check", format!("\"Check {size} pt\""), "toggle"),
            ] {
                let r = find_ref(&text, &label)?;
                let target = format!("{kind}-{size}");
                for _ in 0..reps {
                    let (res, t0) = self.act(
                        id,
                        Action::Click {
                            target: crate::action::Target {
                                r#ref: Some(r.clone()),
                                ..Default::default()
                            },
                            button: Default::default(),
                            count: 1,
                            modifiers: Vec::new(),
                            expect: None,
                        },
                    )?;
                    self.score(&format!("P1 press {target}"), &res, t0, &target, |e| {
                        e.id == target && e.ev == ev
                    })?;
                }
            }
        }
        let slider = find_ref(&text, "slider \"Level\"")?;
        for i in 0..reps {
            let want = (i % 100).to_string();
            let (res, t0) = self.act(
                id,
                Action::SetValue {
                    r#ref: slider.clone(),
                    text: want.clone(),
                    expect: None,
                },
            )?;
            // A slider set through accessibility sends its action: the fixture logs the value.
            self.score("S3 set_value slider", &res, t0, "slider", |e| {
                e.id == "slider" && e.v.as_deref() == Some(want.as_str())
            })?;
        }
        for _ in 0..reps {
            let (res, t0) = self.act(
                id,
                Action::Menu {
                    path: vec!["Targets".into(), "Plain Item".into()],
                    expect: None,
                },
            )?;
            self.score("S3 pick menu-bar", &res, t0, "menu", |e| {
                e.id == "menu" && e.v.as_deref() == Some("Plain Item")
            })?;
        }
        let popup = find_ref(&text, "popup \"Letter\"")?;
        let letters = ["Alpha", "Beta", "Gamma", "Delta"];
        for i in 0..reps as usize {
            let want = letters[(i + 1) % letters.len()];
            let (res, t0) = self.act(
                id,
                Action::SetValue {
                    r#ref: popup.clone(),
                    text: want.into(),
                    expect: None,
                },
            )?;
            self.score("S3 pick popup", &res, t0, "popup", |e| {
                e.id == "popup" && e.v.as_deref() == Some(want)
            })?;
        }
        self.report.coverage.insert(
            "buttons, checkboxes".into(),
            vec!["element (press)".into(), "background pointer".into()],
        );
        self.report
            .coverage
            .insert("slider, pop-up".into(), vec!["element (set value)".into()]);
        Ok(())
    }

    /// Clicks a canvas point given in canvas points, through image `img`. Returns the received
    /// point when the fixture saw the expected dot.
    fn click_dot(
        &mut self,
        op: &str,
        img: &str,
        dot: &str,
        canvas_pt: Point,
        title_bar: f64,
    ) -> Result<()> {
        let t = self
            .engine
            .image_transform(img)
            .context("the image is gone")?;
        let wp = Point::new(
            CANVAS_ORIGIN.0 + canvas_pt.x,
            title_bar + CANVAS_ORIGIN.1 + canvas_pt.y,
        );
        let ip = t.to_image(wp);
        let (res, t0) = self.act(
            self.win.id,
            Action::Click {
                target: crate::action::Target {
                    image: Some(img.into()),
                    x: Some(ip.x),
                    y: Some(ip.y),
                    ..Default::default()
                },
                button: Default::default(),
                count: 1,
                modifiers: Vec::new(),
                expect: None,
            },
        )?;
        let hit = self.score(op, &res, t0, dot, |e| e.id == dot && e.ev == "down")?;
        if let Some(e) = hit
            && let (Some(x), Some(y)) = (e.x, e.y)
        {
            let d = ((x - canvas_pt.x).powi(2) + (y - canvas_pt.y).powi(2)).sqrt();
            self.report.mapping_error_pt.0.push(d);
        } else if res.status == Status::Done {
            self.report.mapping_outside_target += 1;
        }
        // Read the up event too, so it isn't counted against the next click.
        let _ = self
            .log
            .until(Duration::from_millis(200), |e| e.id == dot && e.ev == "up")?;
        Ok(())
    }

    /// S4, P2: background pixel clicks on the dots, centre and four points 1 pt inside the edge,
    /// through the 1× window image and a 2× zoom.
    fn pixels(&mut self, reps: u32) -> Result<()> {
        let id = self.win.id;
        let title_bar = self.win.frame.h - CONTENT_HEIGHT;
        let base = self
            .observe(id, Screenshot::Always, true)?
            .image
            .context("no window image")?
            .id;
        let per_point = (reps / 10).max(1);
        for (dot, size, cx, cy) in DOTS {
            let r = size / 2.0 - 1.0;
            let d = r / std::f64::consts::SQRT_2;
            let points = [
                Point::new(cx, cy),
                Point::new(cx - d, cy - d),
                Point::new(cx + d, cy - d),
                Point::new(cx - d, cy + d),
                Point::new(cx + d, cy + d),
            ];
            // The 2× image: a zoom around the dot.
            let t = self.engine.image_transform(&base).context("base image")?;
            let c = t.to_image(Point::new(
                CANVAS_ORIGIN.0 + cx,
                title_bar + CANVAS_ORIGIN.1 + cy,
            ));
            let zoom = self
                .engine
                .zoom(
                    "bench",
                    &crate::action::ZoomRequest {
                        image: base.clone(),
                        region: [c.x - 40.0, c.y - 40.0, c.x + 40.0, c.y + 40.0],
                    },
                )
                .map_err(|e| anyhow!("zoom: {e}"))?
                .image
                .context("no zoom image")?
                .id;
            for img in [&base, &zoom] {
                let scale = if img == &base { "1x" } else { "2x" };
                for p in points {
                    for _ in 0..per_point {
                        self.click_dot(&format!("P2 click {dot} {scale}"), img, dot, p, title_bar)?;
                    }
                }
            }
        }
        self.report.coverage.insert(
            "canvas dots (no accessibility, refuses the first click)".into(),
            vec!["background pointer with synthetic activation".into()],
        );
        Ok(())
    }

    /// P2r: a minimised window can't take a background click; it must say so (the foreground
    /// rung, off here, is P2f's).
    fn refusals(&mut self, reps: u32) -> Result<()> {
        self.engine.foreground = false;
        let id = self.mini.id;
        let text = self.observe(id, Screenshot::Never, true)?.text;
        let win_ref = find_ref(&text, "window \"Minimised Target\"")?;
        for _ in 0..reps {
            let (res, _) = self.act(
                id,
                Action::Click {
                    target: crate::action::Target {
                        r#ref: Some(win_ref.clone()),
                        ..Default::default()
                    },
                    button: Default::default(),
                    count: 1,
                    modifiers: Vec::new(),
                    expect: None,
                },
            )?;
            let o = self
                .report
                .ops
                .entry("P2r refuse minimised".into())
                .or_default();
            o.tried += 1;
            match res.error.as_ref().map(|e| e.code) {
                Some(ErrorCode::BackgroundUnavailable) => o.ok += 1,
                Some(other) => o.miss(other.as_str()),
                None => o.miss("no refusal"),
            }
            let events = self.log.read_new()?;
            for e in events.into_iter().filter(|e| e.id != "app") {
                self.report
                    .wrong_target
                    .push(("P2r click on a minimised window".into(), e));
            }
        }
        self.report.coverage.insert(
            "minimised window".into(),
            vec!["refused: background_unavailable".into()],
        );
        self.engine.foreground = true;
        Ok(())
    }

    /// P2f: the foreground rung, live, only when nobody has used the computer for a minute. A
    /// click on the minimised window raises it and lands, the window goes back to the Dock and
    /// the front back to the user's app (F1 checks the rest of the user's side).
    fn foreground_rung(&mut self) -> Result<()> {
        let idle = (self.engine.desktop.idle_source())();
        if idle < crate::engine::FOREGROUND_IDLE.as_secs_f64() {
            self.report.coverage.insert(
                "foreground rung".into(),
                vec![format!("not run: the computer was used {idle:.0} s ago")],
            );
            return Ok(());
        }
        let before = self.engine.desktop.user_focus();
        let id = self.mini.id;
        let text = self.observe(id, Screenshot::Never, true)?.text;
        let win_ref = find_ref(&text, "window \"Minimised Target\"")?;
        let (res, _) = self.act(
            id,
            Action::Click {
                target: crate::action::Target {
                    r#ref: Some(win_ref),
                    ..Default::default()
                },
                button: Default::default(),
                count: 1,
                modifiers: Vec::new(),
                expect: None,
            },
        )?;
        std::thread::sleep(Duration::from_millis(200));
        // The window's centre, title bar included, falls just above the dot: any press on its
        // canvas counts.
        let hit = self
            .log
            .read_new()?
            .iter()
            .any(|e| (e.id == "mini-dot" || e.id == "canvas") && e.ev == "down");
        let front_back = self.engine.desktop.user_focus().frontmost_pid == before.frontmost_pid;
        // The window flies back into the Dock for a moment.
        let end = Instant::now() + Duration::from_secs(2);
        let mut minimised = false;
        while !minimised && Instant::now() < end {
            minimised = self.engine.desktop.window(id).is_ok_and(|w| w.minimized);
            std::thread::sleep(Duration::from_millis(50));
        }
        let o = self
            .report
            .ops
            .entry("P2f foreground rung".into())
            .or_default();
        o.tried += 1;
        if res.delivered == Some(Rung::Foreground) && hit && front_back && minimised {
            o.ok += 1;
        } else {
            o.miss(format!(
                "rung {:?}, hit {hit}, front back {front_back}, minimised again {minimised}, {:?}",
                res.delivered, res.error
            ));
        }
        self.report.coverage.insert(
            "foreground rung".into(),
            vec![format!(
                "after {idle:.0} s idle: raised, clicked, front given back {front_back}, minimised again {minimised}"
            )],
        );
        Ok(())
    }

    /// S5: 100 characters, through the value (one call) and through key events.
    fn typing(&mut self, reps: u32) -> Result<()> {
        let id = self.win.id;
        let text = self.observe(id, Screenshot::Never, true)?.text;
        let notes = find_ref(&text, "textfield \"Notes\"")?;
        let hundred: String = "The quick brown fox jumps over the lazy dog. "
            .chars()
            .cycle()
            .take(100)
            .collect();
        for _ in 0..reps {
            let (res, _) = self.act(
                id,
                Action::SetValue {
                    r#ref: notes.clone(),
                    text: hundred.clone(),
                    expect: None,
                },
            )?;
            // A value set through accessibility sends no edit notice; the engine reads it back.
            let o = self
                .report
                .ops
                .entry("S5 type 100 set-value".into())
                .or_default();
            o.tried += 1;
            if res.status == Status::Done {
                o.ok += 1;
                o.dispatch
                    .0
                    .push(res.timings.checks_ms + res.timings.dispatch_ms);
                o.quiet.0.push(res.timings.settle_ms);
            } else {
                o.miss(res.error.map_or("failed", |e| e.code.as_str()));
            }
            let _ = self.log.read_new()?;
        }
        // Key events: the field takes the focus, is cleared, then typed into.
        for _ in 0..reps {
            let (res, _) = self.act(
                id,
                Action::SetValue {
                    r#ref: notes.clone(),
                    text: String::new(),
                    expect: None,
                },
            )?;
            if res.status != Status::Done {
                bail!("couldn't clear the notes field");
            }
            let notes_el = self
                .engine
                .ref_element(id, tree::parse_ref(&notes).unwrap_or(0))
                .context("notes element")?;
            self.engine
                .desktop
                .set_focus(&notes_el)
                .map_err(|e| anyhow!("{e}"))?;
            let _ = self.log.read_new()?;
            let before = self.engine.desktop.user_focus();
            let t0 = epoch_ms();
            let start = Instant::now();
            let cancel = self.engine.gens.token("bench", Duration::from_secs(30));
            self.engine
                .desktop
                .type_text(self.win.pid, &hundred, &cancel)
                .map_err(|e| anyhow!("{e}"))?;
            let dispatch = start.elapsed().as_secs_f64() * 1000.0;
            let want = hundred.clone();
            let events = self.log.until(Duration::from_millis(2_000), |e| {
                e.id == "notes" && e.v.as_deref() == Some(want.as_str())
            })?;
            let o = self
                .report
                .ops
                .entry("S5 type 100 keys".into())
                .or_default();
            o.tried += 1;
            o.dispatch.0.push(dispatch);
            match events
                .iter()
                .find(|e| e.id == "notes" && e.v.as_deref() == Some(hundred.as_str()))
            {
                Some(e) => {
                    o.ok += 1;
                    o.effect.0.push(ms_since(t0, e.t));
                }
                None => o.miss(format!(
                    "the field doesn't hold the text (last: {:?})",
                    events
                        .iter()
                        .rev()
                        .find(|e| e.id == "notes")
                        .and_then(|e| e.v.clone())
                )),
            }
            self.check_focus("type 100 characters as key events", &before);
        }
        Ok(())
    }

    /// `select` then `type`: characters 6–11 of the notes field are replaced, checked in the
    /// fixture's own log.
    fn selection(&mut self, reps: u32) -> Result<()> {
        let id = self.win.id;
        let text = self.observe(id, Screenshot::Never, true)?.text;
        let notes = find_ref(&text, "textfield \"Notes\"")?;
        for _ in 0..reps {
            self.act(
                id,
                Action::SetValue {
                    r#ref: notes.clone(),
                    text: "hello world!".into(),
                    expect: None,
                },
            )?;
            let _ = self.log.read_new()?;
            let before = self.engine.desktop.user_focus();
            let reply = self
                .engine
                .act(
                    "bench",
                    &ActRequest {
                        window: id,
                        actions: vec![
                            Action::Select {
                                r#ref: notes.clone(),
                                start: 6,
                                length: 5,
                                expect: None,
                            },
                            Action::Type {
                                text: "there".into(),
                                r#ref: None,
                                expect: None,
                            },
                        ],
                        screenshot: Screenshot::Never,
                    },
                )
                .map_err(|e| anyhow!("{e}"))?;
            self.logged(reply.trajectory.clone())?;
            let events = self.log.until(Duration::from_millis(1_000), |e| {
                e.id == "notes" && e.v.as_deref() == Some("hello there!")
            })?;
            let o = self
                .report
                .ops
                .entry("SEL select and type over".into())
                .or_default();
            o.tried += 1;
            let done = reply.results.iter().all(|r| r.status == Status::Done);
            if done
                && events
                    .iter()
                    .any(|e| e.v.as_deref() == Some("hello there!"))
            {
                o.ok += 1;
            } else {
                o.miss(format!(
                    "{:?} / last {:?}",
                    reply
                        .results
                        .iter()
                        .map(|r| r.error.as_ref().map(|e| e.code))
                        .collect::<Vec<_>>(),
                    events
                        .iter()
                        .rev()
                        .find(|e| e.id == "notes")
                        .and_then(|e| e.v.clone())
                ));
            }
            self.check_focus("select and type over", &before);
        }
        Ok(())
    }

    fn gates(&mut self) {
        let mut gates = Vec::new();
        let ops = &self.report.ops;
        let pct = |op: &str, f: fn(&Op) -> &Samples, p: f64| {
            ops.get(op)
                .and_then(|o| f(o).pct(p))
                .unwrap_or(f64::INFINITY)
        };
        let all = |prefix: &str, f: fn(&Op) -> &Samples| -> Samples {
            Samples(
                ops.iter()
                    .filter(|(k, _)| k.starts_with(prefix))
                    .flat_map(|(_, o)| f(o).0.clone())
                    .collect(),
            )
        };
        let rate = |prefix: &str| -> (u32, u32) {
            ops.iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .fold((0, 0), |(a, b), (_, o)| (a + o.ok, b + o.tried))
        };
        let lim = |a: f64, b: f64| a <= b;
        for (gid, op, p50, p95) in [
            ("S1", "S1 observe", 15.0, 40.0),
            ("S2", "S2 observe+shot", 70.0, 120.0),
        ] {
            let (a, b) = (
                pct(op, |o| &o.dispatch, 50.0),
                pct(op, |o| &o.dispatch, 95.0),
            );
            gates.push(Gate {
                id: gid,
                what: op.into(),
                measured: format!("{a:.1} / {b:.1} ms"),
                target: format!("≤ {p50} / ≤ {p95} ms"),
                pass: lim(a, p50) && lim(b, p95),
            });
        }
        for op in ["S3 set_value slider", "S3 pick menu-bar"] {
            let d = pct(op, |o| &o.dispatch, 50.0);
            let e50 = pct(op, |o| &o.effect, 50.0);
            let e95 = pct(op, |o| &o.effect, 95.0);
            let (ok, n) = rate(op);
            gates.push(Gate {
                id: "S3",
                what: op.into(),
                measured: format!("dispatch {d:.1} · effect {e50:.1} / {e95:.1} ms ({ok}/{n})"),
                target: "dispatch ≤ 10 · effect ≤ 40 / ≤ 150 ms".into(),
                pass: d <= 10.0 && e50 <= 40.0 && e95 <= 150.0 && n > 0 && ok == n,
            });
        }
        // A pop-up's pick waits out AppKit's ≈350 ms blink of the chosen item, a platform limit
        // with no background route around it (§4.4): its own target, effect only.
        let e50 = pct("S3 pick popup", |o| &o.effect, 50.0);
        let e95 = pct("S3 pick popup", |o| &o.effect, 95.0);
        let (ok, n) = rate("S3 pick popup");
        gates.push(Gate {
            id: "S3p",
            what: "pick a pop-up item".into(),
            measured: format!("effect {e50:.1} / {e95:.1} ms ({ok}/{n})"),
            target: "effect ≤ 400 / ≤ 400 ms, all landed".into(),
            pass: e95 <= 400.0 && n > 0 && ok == n,
        });
        let press_d = all("P1 press", |o| &o.dispatch);
        let press_e = all("P1 press", |o| &o.effect);
        let (d, e50, e95) = (
            press_d.pct(50.0).unwrap_or(f64::INFINITY),
            press_e.pct(50.0).unwrap_or(f64::INFINITY),
            press_e.pct(95.0).unwrap_or(f64::INFINITY),
        );
        gates.push(Gate {
            id: "S3",
            what: "press (all P1 targets)".into(),
            measured: format!("dispatch {d:.1} · effect {e50:.1} / {e95:.1} ms"),
            target: "dispatch ≤ 10 · effect ≤ 40 / ≤ 150 ms".into(),
            pass: d <= 10.0 && e50 <= 40.0 && e95 <= 150.0,
        });
        let click_d = all("P2 click", |o| &o.dispatch);
        let click_e = all("P2 click", |o| &o.effect);
        let (d, e50, e95) = (
            click_d.pct(50.0).unwrap_or(f64::INFINITY),
            click_e.pct(50.0).unwrap_or(f64::INFINITY),
            click_e.pct(95.0).unwrap_or(f64::INFINITY),
        );
        gates.push(Gate {
            id: "S4",
            what: "background pixel click (all P2 clicks)".into(),
            measured: format!("dispatch {d:.1} · effect {e50:.1} / {e95:.1} ms"),
            target: "dispatch ≤ 15 · effect ≤ 60 / ≤ 200 ms".into(),
            pass: d <= 15.0 && e50 <= 60.0 && e95 <= 200.0,
        });
        let sv = pct("S5 type 100 set-value", |o| &o.dispatch, 50.0);
        let keys = pct("S5 type 100 keys", |o| &o.effect, 50.0);
        let (sv_ok, sv_n) = rate("S5 type 100 set-value");
        let (k_ok, k_n) = rate("S5 type 100 keys");
        gates.push(Gate {
            id: "S5",
            what: "type 100 characters: set value / key events (p50)".into(),
            measured: format!("{sv:.1} / {keys:.1} ms ({sv_ok}/{sv_n}, {k_ok}/{k_n} landed)"),
            target: "≤ 20 / ≤ 250 ms".into(),
            pass: sv <= 20.0 && keys <= 250.0 && sv_ok == sv_n && k_ok == k_n,
        });
        let (ok, n) = rate("SEL");
        gates.push(Gate {
            id: "SEL",
            what: "select characters 6–11 and type over them".into(),
            measured: format!("{ok}/{n}"),
            target: "100%".into(),
            pass: n > 0 && ok == n,
        });
        let (ok, n) = rate("P1 press");
        gates.push(Gate {
            id: "P1",
            what: "element-path success, 8/12/16/24 pt".into(),
            measured: format!("{ok}/{n}"),
            target: "100%".into(),
            pass: n > 0 && ok == n,
        });
        let (ok, n) = rate("P2 click");
        let worst = self
            .report
            .mapping_error_pt
            .0
            .iter()
            .copied()
            .fold(0.0, f64::max);
        gates.push(Gate {
            id: "P2",
            what: "pixel-path mapping, 1× and 2×, centre and inset points".into(),
            measured: format!("{ok}/{n} inside the target · worst error {worst:.2} pt"),
            target: "100% inside, ≤ 0.5 pt".into(),
            pass: n > 0 && ok == n && worst <= 0.5 && self.report.mapping_outside_target == 0,
        });
        let (ok, n) = rate("P2r");
        gates.push(Gate {
            id: "P2r",
            what: "refusals say background_unavailable".into(),
            measured: format!("{ok}/{n}"),
            target: "100%".into(),
            pass: n > 0 && ok == n,
        });
        // Only when the computer sat idle for a minute; otherwise it isn't a gate this run.
        let (ok, n) = rate("P2f");
        if n > 0 {
            gates.push(Gate {
                id: "P2f",
                what: "foreground rung: raised, clicked, front and Dock given back".into(),
                measured: format!("{ok}/{n}"),
                target: "100%".into(),
                pass: ok == n,
            });
        }
        gates.push(Gate {
            id: "P4",
            what: "wrong-target actions with an effect".into(),
            measured: self.report.wrong_target.len().to_string(),
            target: "0".into(),
            pass: self.report.wrong_target.is_empty(),
        });
        gates.push(Gate {
            id: "F1",
            what: "focus theft: frontmost app, cursor, key windows".into(),
            measured: self.report.focus_changes.len().to_string(),
            target: "0".into(),
            pass: self.report.focus_changes.is_empty(),
        });
        self.report.gates = gates;
    }

    fn table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{:<32} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
            "operation", "ok", "chk p50", "disp p50", "disp p95", "eff p50", "eff p95", "quiet p50"
        );
        let f = |v: Option<f64>| v.map_or("-".into(), |v| format!("{v:.1}"));
        for (k, o) in &self.report.ops {
            let _ = writeln!(
                out,
                "{:<32} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
                k,
                format!("{}/{}", o.ok, o.tried),
                f(o.checks.pct(50.0)),
                f(o.dispatch.pct(50.0)),
                f(o.dispatch.pct(95.0)),
                f(o.effect.pct(50.0)),
                f(o.effect.pct(95.0)),
                f(o.quiet.pct(50.0)),
            );
            for (why, n) in &o.misses {
                let _ = writeln!(out, "    miss ×{n}: {why}");
            }
        }
        let _ = writeln!(
            out,
            "\nT1 fixture structure text: {} tokens · T2 window image: {}",
            self.report.t1_text_tokens, self.report.t2_image
        );
        let _ = writeln!(
            out,
            "\n{:<5} {:<52} {:<44} {:<36} result",
            "gate", "what", "measured", "target"
        );
        for g in &self.report.gates {
            let _ = writeln!(
                out,
                "{:<5} {:<52} {:<44} {:<36} {}",
                g.id,
                g.what,
                g.measured,
                g.target,
                if g.pass { "pass" } else { "MISS" }
            );
        }
        for (a, e) in &self.report.wrong_target {
            let _ = writeln!(out, "wrong target: {a} → {} {}", e.id, e.ev);
        }
        for c in &self.report.focus_changes {
            let _ = writeln!(out, "focus changed: {c}");
        }
        for c in &self.report.outside_focus_changes {
            let _ = writeln!(out, "not counted: {c}");
        }
        out
    }
}

/// The fixture process (pid, start time): killed by its own pid when dropped, while that pid
/// is still the fixture.
struct Fixture(i32, Option<u64>);

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.1.is_none() || crate::macos::process_start_us(self.0) != self.1 {
            return;
        }
        let _ = Command::new("/bin/kill")
            .args(["-9", &self.0.to_string()])
            .stderr(Stdio::null())
            .status();
    }
}

fn wait_windows(desktop: &mut MacDesktop, pid: i32) -> Result<(WindowInfo, WindowInfo)> {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        let wins = desktop.windows(pid).unwrap_or_default();
        let main = wins.iter().find(|w| w.title == "Target Range");
        let mini = wins
            .iter()
            .find(|w| w.title == "Minimised Target")
            .and_then(|w| desktop.window(w.id).ok())
            .filter(|w| w.minimized);
        if let (Some(a), Some(b)) = (main, mini) {
            return Ok((a.clone(), b));
        }
        if Instant::now() > end {
            bail!(
                "the fixture's windows didn't appear: {:?}",
                wins.iter()
                    .map(|w| (w.id, &w.title, desktop.window(w.id).map(|x| x.minimized)))
                    .collect::<Vec<_>>()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Runs the bench. With a `cursor`, the agent cursor draws every action it aims; with `replay`,
/// the run's action log and its images are written there for the app's timeline.
pub fn run(
    mut desktop: MacDesktop,
    out: &Path,
    quick: bool,
    foreground: bool,
    cursor: Cursor,
    replay: Option<&Path>,
) -> Result<bool> {
    let reps = if quick { 20 } else { 200 };
    std::fs::create_dir_all(out)?;
    let out = out.canonicalize()?;
    let log_path = out.join("fixture-log.jsonl");
    let user = desktop.user_focus();
    let pid = crate::suite_run::launch_fixture("target-range", &[log_path.as_os_str()])?;
    let fixture = Fixture(pid, crate::macos::process_start_us(pid));
    let (win, mini) = wait_windows(&mut desktop, pid)?;
    // The fixture's own window notices settle before the clock starts.
    std::thread::sleep(Duration::from_millis(500));
    let mut engine = Engine::new(desktop, crate::harness::dev_block_list(), Provider::Claude);
    engine.desktop.watch(pid);
    if let Some(c) = &cursor {
        c.label("bench", "Bench");
    }
    engine.cursor = cursor;
    let mut b = Bench {
        engine,
        log: LogTail {
            path: log_path,
            offset: 0,
        },
        win,
        mini,
        report: Report {
            reps,
            ..Default::default()
        },
        user,
        replay: match replay {
            Some(dir) => {
                std::fs::create_dir_all(dir)?;
                Some(ReplayLog {
                    dir: dir.to_owned(),
                    actions: Vec::new(),
                    batches: 0,
                })
            }
            None => None,
        },
    };
    let _ = b.log.read_new()?;
    let started = Instant::now();
    b.observations(reps)?;
    b.elements(reps)?;
    b.pixels(reps)?;
    b.refusals(reps)?;
    b.typing(reps)?;
    b.selection((reps / 10).max(5))?;
    let start_focus = b.user.clone();
    b.check_focus("the whole run", &start_focus);
    if foreground {
        b.foreground_rung()?;
    } else {
        b.report.coverage.insert(
            "foreground rung".into(),
            vec!["not run: --no-foreground".into()],
        );
    }
    drop(fixture);
    b.gates();
    let table = b.table();
    print!("{table}");
    println!(
        "{} reps per operation · {:.0} s · terminal-launched development evidence",
        reps,
        started.elapsed().as_secs_f64()
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let json = out.join(format!("bench-{stamp}.json"));
    std::fs::write(&json, serde_json::to_vec_pretty(&b.report)?)?;
    std::fs::write(out.join(format!("bench-{stamp}.txt")), &table)?;
    println!("results in {}", json.display());
    if let Some(log) = &b.replay {
        println!("action log in {}", log.write()?.display());
    }
    Ok(b.report.gates.iter().all(|g| g.pass))
}
