//! `launch` (§4.6): opens an app, a file or a URL in the background and tells what it opened
//! apart from what was already there (§5, ownership). The system may hand back an app the user
//! already runs; that one is never the worker's to quit.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::block::{TERMINAL_NOT_LAUNCHED, TargetFacts};
use crate::cancel::CancelToken;
use crate::desktop::{AppInfo, Desktop};
use crate::engine::Engine;
use crate::error::{CuError, CuResult, ErrorCode, err};
use crate::wire::LaunchRequest;

/// How long a launch waits for its window.
const WINDOW_WAIT: Duration = Duration::from_secs(10);
/// How long an app that was already running gets to show a window before `launch` returns.
const REUSED_GRACE: Duration = Duration::from_millis(800);

/// What a launch opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub app: AppInfo,
    pub new_process: bool,
    pub new_windows: Vec<u32>,
    pub front_restored: bool,
}

/// Whether `app` (a name, bundle id or path, as the request gave it) names this app.
fn names(want: &str, a: &AppInfo) -> bool {
    let want = want.trim_end_matches('/');
    let stem = |p: &str| {
        p.trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".app")
            .to_owned()
    };
    a.name.eq_ignore_ascii_case(want)
        || a.bundle_id
            .as_deref()
            .is_some_and(|b| b.eq_ignore_ascii_case(want))
        || a.bundle_path.as_deref().is_some_and(|p| {
            p.trim_end_matches('/') == want || stem(p).eq_ignore_ascii_case(&stem(want))
        })
}

fn shown(a: &AppInfo) -> HashSet<u32> {
    a.windows
        .iter()
        .filter(|w| w.on_screen || w.minimized)
        .map(|w| w.id)
        .collect()
}

pub fn launch<D: Desktop>(
    engine: &mut Engine<D>,
    req: &LaunchRequest,
    cancel: &CancelToken,
) -> CuResult<Opened> {
    if req.app.is_none() && req.open.is_none() {
        return err(
            ErrorCode::BadRequest,
            "give an app, a file or URL to open, or both",
        );
    }
    // Refused before anything opens; a terminal is allowed, as long as it's a new one.
    if let Some(app) = req.app.as_deref() {
        let facts = TargetFacts {
            pid: -1,
            bundle_id: Some(app),
            app_name: app,
            bundle_path: Some(app),
            ..Default::default()
        };
        if let Some(reason) = engine
            .block
            .check(&facts)
            .filter(|r| *r != TERMINAL_NOT_LAUNCHED)
        {
            return err(ErrorCode::Blocked, reason);
        }
    }
    let before: HashMap<i32, HashSet<u32>> = engine
        .desktop
        .apps()?
        .iter()
        .map(|a| (a.pid, shown(a)))
        .collect();
    let front_before = engine.desktop.user_focus().frontmost_pid;
    engine
        .desktop
        .open(req.app.as_deref(), req.open.as_deref())?;
    let started = Instant::now();
    let found = loop {
        cancel.check()?;
        let apps = engine.desktop.apps()?;
        let fresh = |a: &AppInfo| {
            before
                .get(&a.pid)
                .is_none_or(|old| shown(a).difference(old).next().is_some())
        };
        let pick = match req.app.as_deref() {
            Some(want) => {
                let named: Vec<&AppInfo> = apps.iter().filter(|a| names(want, a)).collect();
                named
                    .iter()
                    .find(|a| !before.contains_key(&a.pid))
                    .or_else(|| named.iter().find(|a| fresh(a)))
                    .or_else(|| named.first())
                    .map(|a| (*a).clone())
            }
            // A file or URL alone: whatever app started or showed a new window for it.
            None => apps
                .iter()
                .find(|a| !before.contains_key(&a.pid) && !a.windows.is_empty())
                .or_else(|| {
                    apps.iter()
                        .find(|a| before.contains_key(&a.pid) && fresh(a))
                })
                .cloned(),
        };
        let waited = started.elapsed();
        if let Some(a) = pick {
            let new_process = !before.contains_key(&a.pid);
            let new_windows: Vec<u32> = match before.get(&a.pid) {
                Some(old) => shown(&a).difference(old).copied().collect(),
                None => shown(&a).into_iter().collect(),
            };
            let reused_done = !new_process && req.open.is_none() && waited >= REUSED_GRACE;
            if !new_windows.is_empty() || reused_done || waited >= WINDOW_WAIT {
                let mut new_windows = new_windows;
                new_windows.sort_unstable();
                break Opened {
                    app: a,
                    new_process,
                    new_windows,
                    front_restored: false,
                };
            }
        } else if waited >= WINDOW_WAIT {
            return err(ErrorCode::NoSuchTarget, "nothing opened a window for that");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut opened = found;
    // The launch took the front: give it back to the user's app (§2.1).
    let front_now = engine.desktop.user_focus().frontmost_pid;
    if front_now == opened.app.pid && front_before != opened.app.pid && front_before != 0 {
        opened.front_restored = engine.desktop.activate(front_before).is_ok();
    }
    let facts = TargetFacts {
        pid: opened.app.pid,
        bundle_id: opened.app.bundle_id.as_deref(),
        app_name: &opened.app.name,
        bundle_path: opened.app.bundle_path.as_deref(),
        ..Default::default()
    };
    if let Some(reason) = engine.block.check(&facts) {
        // A terminal this launch started is the session's; any other blocked app isn't.
        if !(reason == TERMINAL_NOT_LAUNCHED && opened.new_process) {
            return Err(CuError::new(ErrorCode::Blocked, reason));
        }
    }
    Ok(opened)
}
