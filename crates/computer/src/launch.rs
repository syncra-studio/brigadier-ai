//! `launch` (§4.6): opens an app, a file or a URL in the background and tells what it opened
//! apart from what was already there (§5, ownership). The system may hand back an app the user
//! already runs; that one is never the worker's to quit.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::block::{TERMINAL_NOT_LAUNCHED, TargetFacts};
use crate::cancel::CancelToken;
use crate::desktop::{AppInfo, Desktop, WindowInfo};
use crate::engine::Engine;
use crate::error::{CuError, CuResult, ErrorCode, err};
use crate::wire::LaunchRequest;

/// How long a launch waits for its window.
const WINDOW_WAIT: Duration = Duration::from_secs(10);
/// How long an app that was already running gets to show a window before `launch` returns.
const REUSED_GRACE: Duration = Duration::from_millis(800);
/// How long, once windows appeared, a launch that opened a file waits for the one showing it;
/// an app restoring its saved state may show other documents first.
const DOCUMENT_GRACE: Duration = Duration::from_millis(1500);

/// What a launch opened.
#[derive(Debug, Clone, PartialEq)]
pub struct Opened {
    pub app: AppInfo,
    pub new_process: bool,
    /// The windows this launch opened: the one showing the file when it opened one and the
    /// window can be told.
    pub new_windows: Vec<u32>,
    /// Windows that appeared alongside, which the app reopened from its saved state: the
    /// user's documents, not this launch's.
    pub restored_windows: Vec<u32>,
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

/// A local path or `file:` URL as a plain path, with symlinks such as `/tmp` resolved.
fn local_path(s: &str) -> Option<PathBuf> {
    let path = match s.strip_prefix("file://") {
        Some(rest) => percent_decode(rest.strip_prefix("localhost").unwrap_or(rest)),
        None if s.contains("://") => return None,
        None => s.to_owned(),
    };
    let path = PathBuf::from(path.trim_end_matches('/'));
    path.is_absolute()
        .then(|| std::fs::canonicalize(&path).unwrap_or(path))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && let (Some(h), Some(l)) = (
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether window `w` shows `file`: by the document the app reports for it, else by a title
/// that is the file's name, with or without its extension.
fn shows<D: Desktop>(desktop: &mut D, w: &WindowInfo, file: &Path) -> bool {
    if let Some(doc) = desktop.document(w) {
        return local_path(&doc).is_some_and(|p| p == file);
    }
    let title = w.title.trim();
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let stem = file.file_stem().and_then(|n| n.to_str()).unwrap_or("");
    !title.is_empty() && (title == name || title == stem)
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
    let file = req.open.as_deref().and_then(local_path);
    let started = Instant::now();
    let mut first_window: Option<Instant> = None;
    let found = loop {
        // Ended after `open` ran: an app it started is still reported, so it's owned and
        // cleaned up; anything else ends here.
        let stopped = cancel.check().err();
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
        if let Some(e) = &stopped
            && pick.as_ref().is_none_or(|a| before.contains_key(&a.pid))
        {
            return Err(e.clone());
        }
        if let Some(a) = pick {
            let new_process = !before.contains_key(&a.pid);
            let mut appeared: Vec<u32> = match before.get(&a.pid) {
                Some(old) => shown(&a).difference(old).copied().collect(),
                None => shown(&a).into_iter().collect(),
            };
            appeared.sort_unstable();
            let reused_done = !new_process && req.open.is_none() && waited >= REUSED_GRACE;
            if !appeared.is_empty() || reused_done || waited >= WINDOW_WAIT || stopped.is_some() {
                // A file's own window among them; the rest the app restored.
                let (mut new_windows, mut restored_windows) = (appeared.clone(), Vec::new());
                if let Some(file) = &file {
                    let windows: Vec<WindowInfo> = a
                        .windows
                        .iter()
                        .filter(|w| appeared.contains(&w.id))
                        .cloned()
                        .collect();
                    let (own, rest): (Vec<WindowInfo>, Vec<WindowInfo>) = windows
                        .into_iter()
                        .partition(|w| shows(&mut engine.desktop, w, file));
                    if !own.is_empty() {
                        new_windows = own.iter().map(|w| w.id).collect();
                        restored_windows = rest.iter().map(|w| w.id).collect();
                    } else if !appeared.is_empty() {
                        let since = *first_window.get_or_insert_with(Instant::now);
                        if since.elapsed() < DOCUMENT_GRACE
                            && waited < WINDOW_WAIT
                            && stopped.is_none()
                        {
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                    }
                }
                break Opened {
                    app: a,
                    new_process,
                    new_windows,
                    restored_windows,
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
