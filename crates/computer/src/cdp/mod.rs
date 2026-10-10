//! Pages in a Chromium browser the session launched, through the browser's own debugging
//! protocol (§8, Phase 5): structure and actions reach the page itself, so a page needs no
//! pixels, no synthetic activation and never the user's focus. Pixels stay the last rung.
//!
//! The browser runs on a scratch profile with its debugging port on the loopback interface and
//! no window at start; each window is made in the background by a protocol call, so nothing
//! takes the front. The user's own browser profile is never opened this way: a page in a
//! browser the user runs is read through its accessibility web area instead.
//!
//! Pure Rust over a local socket: the same code serves every operating system.

mod conn;
pub mod image;
pub mod input;
pub mod keys;
pub mod page;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub use conn::{Conn, Event};
pub use page::{Dialog, Page, PageNode, Snapshot, Viewport, WebEl};

use crate::desktop::WindowInfo;
use crate::error::{CuError, CuResult, ErrorCode, err};
use crate::geom::Rect;

/// Chromium browsers by bundle id: a `launch` of one starts a session browser.
pub const CHROMIUM_BUNDLES: &[&str] = &[
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.google.Chrome.dev",
    "com.google.Chrome.canary",
    "com.google.chrome.for.testing",
    "org.chromium.Chromium",
    "com.microsoft.edgemac",
    "com.brave.Browser",
    "com.vivaldi.Vivaldi",
];

pub fn is_chromium(bundle_id: Option<&str>) -> bool {
    bundle_id.is_some_and(|b| CHROMIUM_BUNDLES.iter().any(|c| c.eq_ignore_ascii_case(b)))
}

/// The browser's arguments: a scratch profile, the debugging port on a free loopback port, no
/// first-run pages, no window at start, and pages that keep running while hidden or covered.
pub fn launch_args(profile: &Path) -> Vec<String> {
    vec![
        format!("--user-data-dir={}", profile.display()),
        "--remote-debugging-address=127.0.0.1".into(),
        "--remote-debugging-port=0".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--no-startup-window".into(),
        "--disable-backgrounding-occluded-windows".into(),
        "--disable-renderer-backgrounding".into(),
        "--disable-background-timer-throttling".into(),
    ]
}

/// Where the session's browsers keep their scratch profiles: only this helper makes them.
fn profiles_base() -> PathBuf {
    std::env::temp_dir().join("brigadier-browser")
}

/// The scratch profile of a running browser a session launched, maybe from another helper
/// process (the suite's setup, or this helper before a restart), read from its command line.
/// None for any other browser: the user's own are never driven through the protocol.
pub fn adoptable(pid: i32) -> Option<PathBuf> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let line = String::from_utf8_lossy(&out.stdout);
    let base = profiles_base();
    let dir = line
        .split(" --")
        .find_map(|a| a.strip_prefix("user-data-dir="))
        .map(|d| PathBuf::from(d.trim()))?;
    let dir = dir.canonicalize().ok()?;
    let base = base.canonicalize().ok()?;
    (dir.parent() == Some(base.as_path()) && active_port(&dir).is_some()).then_some(dir)
}

/// The browser process running on `profile`, read from the command lines: the main process,
/// not one of its helpers (`--type=…`), which name the profile too.
pub fn started_on(profile: &Path) -> Option<i32> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-ax", "-ww", "-o", "pid=,command="])
        .output()
        .ok()?;
    let want = profile.display().to_string();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|line| {
            let (pid, command) = line.trim_start().split_once(' ')?;
            let args: Vec<&str> = command.split(" --").collect();
            let on = args
                .iter()
                .any(|a| a.strip_prefix("user-data-dir=").map(str::trim) == Some(want.as_str()));
            let helper = args.iter().any(|a| a.starts_with("type="));
            if on && !helper {
                pid.parse().ok()
            } else {
                None
            }
        })
}

/// A fresh scratch profile directory.
pub fn scratch_profile() -> CuResult<PathBuf> {
    let base = profiles_base();
    std::fs::create_dir_all(&base)
        .map_err(|e| CuError::new(ErrorCode::Failed, format!("a browser profile: {e}")))?;
    // Profiles left by a browser that outlived its helper.
    for old in std::fs::read_dir(&base).into_iter().flatten().flatten() {
        let stale = old
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(24 * 3600));
        if stale {
            let _ = std::fs::remove_dir_all(old.path());
        }
    }
    for n in 0..1000u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let p = base.join(format!("{}-{nanos}-{n}", std::process::id()));
        if std::fs::create_dir(&p).is_ok() {
            return Ok(p);
        }
    }
    err(ErrorCode::Failed, "no scratch profile directory")
}

/// The debugging endpoint a browser wrote into its profile: the port, and the browser's path.
pub fn active_port(profile: &Path) -> Option<(u16, String)> {
    let text = std::fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    let mut lines = text.lines();
    let port = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim().to_owned();
    Some((port, path))
}

/// A browser the session launched.
pub struct Browser {
    pub pid: i32,
    /// The app bundle it runs from.
    pub app_path: String,
    /// The worker whose launch started it, the only one a launch opens pages in: another
    /// worker's ends with that worker. `None` for one taken on from another helper.
    pub owner: Option<String>,
    profile: PathBuf,
    conn: Conn,
    pages: HashMap<String, page::Page>,
    /// Which tab each window shows, as last paired.
    windows: HashMap<u32, String>,
    /// The browser's own session-less events are read from here.
    discovered: bool,
}

impl Browser {
    /// Connects to a browser started on `profile`, waiting up to `wait` for its port.
    pub fn attach(pid: i32, app_path: &str, profile: PathBuf, wait: Duration) -> CuResult<Self> {
        let start = Instant::now();
        let (port, path) = loop {
            if let Some(p) = active_port(&profile) {
                break p;
            }
            if start.elapsed() >= wait {
                return err(
                    ErrorCode::AppNotResponding,
                    "the browser didn't open its debugging port",
                );
            }
            std::thread::sleep(Duration::from_millis(30));
        };
        let mut conn = Conn::connect(port, &path)?;
        conn.call(None, "Target.setDiscoverTargets", json!({"discover": true}))?;
        Ok(Self {
            pid,
            app_path: app_path.to_owned(),
            owner: None,
            profile,
            conn,
            pages: HashMap::new(),
            windows: HashMap::new(),
            discovered: true,
        })
    }

    pub fn profile(&self) -> &Path {
        &self.profile
    }

    /// Opens a page in a new window, in the background; returns its target id.
    pub fn new_window(&mut self, url: &str) -> CuResult<String> {
        let r = self.conn.call(
            None,
            "Target.createTarget",
            json!({"url": url, "newWindow": true, "background": true}),
        )?;
        let target = r["targetId"].as_str().unwrap_or_default().to_owned();
        self.page(&target)?;
        Ok(target)
    }

    /// Where a target's window is on the desktop, in global points.
    pub fn window_bounds(&mut self, target: &str) -> CuResult<Rect> {
        let r = self.conn.call(
            None,
            "Browser.getWindowForTarget",
            json!({"targetId": target}),
        )?;
        let b = &r["bounds"];
        let f = |k: &str| b[k].as_f64().unwrap_or(0.0);
        Ok(Rect::new(f("left"), f("top"), f("width"), f("height")))
    }

    /// The page targets: id, title, url.
    fn targets(&mut self) -> CuResult<Vec<(String, String, String)>> {
        let r = self.conn.call(None, "Target.getTargets", json!({}))?;
        Ok(r["targetInfos"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|t| t["type"] == "page")
                    .map(|t| {
                        let s = |k: &str| t[k].as_str().unwrap_or_default().to_owned();
                        (s("targetId"), s("title"), s("url"))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The tab window `w` shows: its bounds match the window's frame, and among tabs of one
    /// window, its title is the window's.
    pub fn pair(&mut self, w: &WindowInfo) -> CuResult<Option<String>> {
        self.absorb();
        if let Some(t) = self.windows.get(&w.id).cloned()
            && self.pages.get(&t).is_some_and(|p| !p.closed)
            && self
                .page_title(&t)
                .is_some_and(|title| title_matches(&title, &w.title))
        {
            return Ok(Some(t));
        }
        let mut same_bounds = Vec::new();
        for (id, title, _) in self.targets()? {
            let Ok(b) = self.window_bounds(&id) else {
                continue;
            };
            if (b.x - w.frame.x).abs() <= 1.0
                && (b.y - w.frame.y).abs() <= 1.0
                && (b.w - w.frame.w).abs() <= 1.0
                && (b.h - w.frame.h).abs() <= 1.0
            {
                same_bounds.push((id, title));
            }
        }
        let pick = match same_bounds.len() {
            0 => None,
            1 => same_bounds.pop().map(|(id, _)| id),
            _ => same_bounds
                .iter()
                .find(|(_, t)| title_matches(t, &w.title))
                .or_else(|| same_bounds.first())
                .map(|(id, _)| id.clone()),
        };
        if let Some(t) = &pick {
            self.page(t)?;
            self.windows.insert(w.id, t.clone());
        }
        Ok(pick)
    }

    fn page_title(&mut self, target: &str) -> Option<String> {
        let r = self
            .conn
            .call(None, "Target.getTargetInfo", json!({"targetId": target}))
            .ok()?;
        r["targetInfo"]["title"].as_str().map(str::to_owned)
    }

    /// The attached page for a target, attaching on first use.
    fn page(&mut self, target: &str) -> CuResult<&mut page::Page> {
        if !self.pages.contains_key(target) {
            let p = page::Page::attach(&mut self.conn, target)?;
            self.pages.insert(target.to_owned(), p);
        }
        self.pages
            .get_mut(target)
            .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, "no such page"))
    }

    /// Reads the events that came in and applies them to the pages.
    pub fn absorb(&mut self) {
        let _ = self.conn.pump(Duration::ZERO);
        let events = self.conn.take(|_| true);
        let mut opened: Vec<(String, String)> = Vec::new();
        for e in events {
            if e.method == "Target.targetCreated" && self.discovered {
                let info = &e.params["targetInfo"];
                if info["type"] == "page"
                    && let (Some(id), Some(opener)) =
                        (info["targetId"].as_str(), info["openerId"].as_str())
                {
                    opened.push((opener.to_owned(), id.to_owned()));
                }
                continue;
            }
            if e.method == "Target.targetDestroyed"
                && let Some(id) = e.params["targetId"].as_str()
                && let Some(p) = self.pages.get_mut(id)
            {
                p.closed = true;
                continue;
            }
            for p in self.pages.values_mut() {
                if p.owns(e.session.as_deref()) {
                    p.apply(&mut self.conn, &e);
                    break;
                }
            }
        }
        for (opener, id) in opened {
            if let Some(p) = self.pages.get_mut(&opener) {
                p.opened.push(id);
            }
        }
    }

    pub fn snapshot(&mut self, target: &str, w: &WindowInfo) -> CuResult<Snapshot> {
        self.absorb();
        let conn = &mut self.conn;
        let p = self
            .pages
            .get_mut(target)
            .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, "no such page"))?;
        p.snapshot(conn, w)
    }

    /// Runs `f` on a page with the connection.
    pub fn with_page<T>(
        &mut self,
        target: &str,
        f: impl FnOnce(&mut page::Page, &mut Conn) -> CuResult<T>,
    ) -> CuResult<T> {
        self.absorb();
        let conn = &mut self.conn;
        let p = self
            .pages
            .get_mut(target)
            .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, "no such page"))?;
        f(p, conn)
    }

    /// Waits for the page to settle: no requests in flight and no DOM change for `quiet`, or a
    /// dialog opened. Returns whether it settled within `bound`.
    pub fn settle(
        &mut self,
        target: &str,
        since: Instant,
        quiet: Duration,
        bound: Duration,
        mut stop: impl FnMut() -> CuResult<()>,
    ) -> CuResult<bool> {
        loop {
            stop()?;
            let _ = self.conn.pump(Duration::from_millis(4));
            self.absorb();
            let conn = &mut self.conn;
            let Some(p) = self.pages.get_mut(target) else {
                return Ok(true);
            };
            if p.dialog.is_some() || p.closed {
                return Ok(true);
            }
            if since.elapsed() >= quiet && p.quiet_for(conn) >= quiet {
                return Ok(true);
            }
            if since.elapsed() >= bound {
                return Ok(false);
            }
        }
    }
}

/// Chromium's window title is the tab's title, maybe with the browser's name after it.
fn title_matches(tab: &str, window: &str) -> bool {
    !tab.is_empty() && (window == tab || window.starts_with(tab))
}

/// The browsers the session launched, and which windows show their pages.
#[derive(Default)]
pub struct Web {
    pub browsers: Vec<Browser>,
    /// Chromium processes looked at and found not to be a session's: not asked again.
    pub foreign: HashSet<i32>,
}

impl Web {
    pub fn add(&mut self, b: Browser) {
        self.browsers.push(b);
    }

    /// Forgets browsers that quit; their profiles go with them. A browser still running when
    /// its helper ends keeps its profile, so another helper can take it on (`adoptable`); a
    /// later start sweeps it once it is a day old.
    pub fn prune(&mut self, mut alive: impl FnMut(i32) -> bool) {
        self.browsers.retain(|b| {
            let on = alive(b.pid);
            if !on {
                let _ = std::fs::remove_dir_all(&b.profile);
            }
            on
        });
    }

    pub fn browser(&mut self, pid: i32) -> Option<&mut Browser> {
        self.browsers.iter_mut().find(|b| b.pid == pid)
    }

    /// The page window `w` shows, when it is a launched browser's: the browser's pid and the
    /// tab's target id.
    pub fn page_of(&mut self, w: &WindowInfo) -> Option<(i32, String)> {
        let b = self.browser(w.pid)?;
        match b.pair(w) {
            Ok(Some(t)) => Some((w.pid, t)),
            _ => None,
        }
    }

    pub fn pids(&self) -> HashSet<i32> {
        self.browsers.iter().map(|b| b.pid).collect()
    }
}

/// A request's JSON, for the protocol's `Runtime.callFunctionOn` returns.
pub(crate) fn by_value(r: &Value) -> Value {
    r["result"]["value"].clone()
}
