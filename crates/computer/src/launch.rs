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
pub(crate) fn local_path(s: &str) -> Option<PathBuf> {
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

/// Whether running app `a` is `r`, the app the system resolved for a launch: by bundle id when
/// both have one, else by bundle path.
fn is_resolved(r: &AppInfo, a: &AppInfo) -> bool {
    match (r.bundle_id.as_deref(), a.bundle_id.as_deref()) {
        (Some(x), Some(y)) => x.eq_ignore_ascii_case(y),
        _ => r
            .bundle_path
            .as_deref()
            .zip(a.bundle_path.as_deref())
            .is_some_and(|(x, y)| x.trim_end_matches('/') == y.trim_end_matches('/')),
    }
}

fn shown(a: &AppInfo) -> HashSet<u32> {
    a.windows
        .iter()
        .filter(|w| w.on_screen || w.minimized)
        .map(|w| w.id)
        .collect()
}

/// The launch took the front for `pid`: gives it back to the user's app (§2.1). Whether it did.
fn give_back_front<D: Desktop>(desktop: &mut D, front_before: i32, pid: i32) -> bool {
    let front_now = desktop.user_focus().frontmost_pid;
    front_now == pid
        && front_before != pid
        && front_before != 0
        && desktop.activate(front_before).is_ok()
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
    // Refused before anything opens, by the name the request gave and by the app the system
    // would run for it; a terminal is allowed, as long as it's a new one.
    let resolved = engine
        .desktop
        .resolve(req.app.as_deref(), req.open.as_deref());
    let mut facts = Vec::new();
    if let Some(app) = req.app.as_deref() {
        facts.push(TargetFacts {
            pid: -1,
            bundle_id: Some(app),
            app_name: app,
            bundle_path: Some(app),
            ..Default::default()
        });
    }
    if let Some(a) = &resolved {
        facts.push(TargetFacts {
            pid: -1,
            bundle_id: a.bundle_id.as_deref(),
            app_name: &a.name,
            bundle_path: a.bundle_path.as_deref(),
            ..Default::default()
        });
    }
    if let Some(reason) = facts
        .iter()
        .filter_map(|f| engine.block.check(f))
        .find(|r| *r != TERMINAL_NOT_LAUNCHED)
    {
        return err(ErrorCode::Blocked, reason);
    }
    // A file or URL alone is told by the app the system runs for it; without one, nothing tells
    // its windows from those the user opens meanwhile.
    if req.app.is_none() && resolved.is_none() {
        return err(ErrorCode::NoSuchTarget, "no app opens that");
    }
    if let Some(app) = resolved
        .as_ref()
        .filter(|a| crate::cdp::is_chromium(a.bundle_id.as_deref()))
        && let Some(path) = app.bundle_path.clone()
    {
        return launch_browser(engine, &path, req.open.as_deref(), cancel);
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
            // A file or URL alone: the app the system runs for it, once it started or showed a
            // new window; other apps the user opens meanwhile aren't this launch's.
            None => {
                let ours: Vec<&AppInfo> = apps
                    .iter()
                    .filter(|a| resolved.as_ref().is_some_and(|r| is_resolved(r, a)))
                    .collect();
                ours.iter()
                    .find(|a| !before.contains_key(&a.pid) && !a.windows.is_empty())
                    .or_else(|| {
                        ours.iter()
                            .find(|a| before.contains_key(&a.pid) && fresh(a))
                    })
                    .map(|a| (*a).clone())
            }
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
                        // A file alone in an app that was running: a window that doesn't show it
                        // may be one the user opened, so only the file's own window will do.
                        // A newly started resolved app keeps the existing grace-period fallback
                        // to all its new windows, preserving ownership when its document can't
                        // be identified. An app bundle is the app itself, not a document.
                        let unproven = req.app.is_none()
                            && !new_process
                            && file.extension().is_none_or(|e| e != "app");
                        let since = *first_window.get_or_insert_with(Instant::now);
                        if (unproven || since.elapsed() < DOCUMENT_GRACE)
                            && waited < WINDOW_WAIT
                            && stopped.is_none()
                        {
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                        if unproven {
                            give_back_front(&mut engine.desktop, front_before, a.pid);
                            return err(
                                ErrorCode::NoSuchTarget,
                                format!("{} showed no window with that file", a.name),
                            );
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
    opened.front_restored = give_back_front(&mut engine.desktop, front_before, opened.app.pid);
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

/// A file path as a URL a browser loads; URLs pass through.
fn as_url(open: &str) -> String {
    match local_path(open) {
        Some(p) if !open.contains("://") => {
            format!(
                "file://{}",
                p.display()
                    .to_string()
                    .replace('%', "%25")
                    .replace(' ', "%20")
            )
        }
        _ => open.to_owned(),
    }
}

/// A Chromium browser for the session (§8, Phase 5): a new instance on a scratch profile with its
/// debugging port, or a new window in the one the session already runs. The window is made in the
/// background by the browser itself, so the browser never takes the front.
fn launch_browser<D: Desktop>(
    engine: &mut Engine<D>,
    path: &str,
    open: Option<&str>,
    cancel: &CancelToken,
) -> CuResult<Opened> {
    open_browser(&mut engine.desktop, &mut engine.web, path, open, cancel)
}

/// A page in a browser of the session's (`web`): a new window in one already running from
/// `path`, or a new instance on a scratch profile with its debugging port. Nothing takes the
/// front: the window is made in the background through the protocol.
pub fn open_browser<D: Desktop>(
    desktop: &mut D,
    web: &mut crate::cdp::Web,
    path: &str,
    open: Option<&str>,
    cancel: &CancelToken,
) -> CuResult<Opened> {
    let url = open.map_or_else(|| "about:blank".to_owned(), as_url);
    web.prune(|pid| desktop.app(pid).is_ok());
    let front_before = desktop.user_focus().frontmost_pid;
    let running = web
        .browsers
        .iter()
        .find(|b| b.app_path.trim_end_matches('/') == path.trim_end_matches('/'))
        .map(|b| b.pid);
    let (pid, new_process) = match running {
        Some(pid) => (pid, false),
        None => {
            let before: HashSet<i32> = desktop.apps()?.iter().map(|a| a.pid).collect();
            let profile = crate::cdp::scratch_profile()?;
            desktop.open_new(path, &crate::cdp::launch_args(&profile))?;
            let started = Instant::now();
            let pid = loop {
                let fresh = desktop.apps()?.into_iter().find(|a| {
                    !before.contains(&a.pid)
                        && a.bundle_path.as_deref().map(|p| p.trim_end_matches('/'))
                            == Some(path.trim_end_matches('/'))
                });
                if let Some(a) = fresh {
                    break a.pid;
                }
                if started.elapsed() >= WINDOW_WAIT {
                    let _ = std::fs::remove_dir_all(&profile);
                    return err(ErrorCode::NoSuchTarget, "the browser didn't start");
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            let browser = crate::cdp::Browser::attach(pid, path, profile, WINDOW_WAIT)?;
            web.add(browser);
            (pid, true)
        }
    };
    // Stopped while the browser started: its page isn't made. A browser this launch started is
    // still reported, so it is owned and quit.
    if let Err(e) = cancel.check() {
        if !new_process {
            return Err(e);
        }
        let mut app = desktop.app(pid)?;
        app.windows = desktop.windows(pid)?;
        return Ok(Opened {
            app,
            new_process,
            new_windows: Vec::new(),
            restored_windows: Vec::new(),
            front_restored: false,
        });
    }
    let browser = web
        .browser(pid)
        .ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, "the browser quit"))?;
    let target = browser.new_window(&url)?;
    let bounds = browser.window_bounds(&target)?;
    // The window the browser made: the one with the tab's bounds.
    let started = Instant::now();
    let window = loop {
        let found = desktop.windows(pid)?.into_iter().find(|w| {
            (w.frame.x - bounds.x).abs() <= 1.0
                && (w.frame.y - bounds.y).abs() <= 1.0
                && (w.frame.w - bounds.w).abs() <= 1.0
                && (w.frame.h - bounds.h).abs() <= 1.0
                && !w.title.is_empty()
        });
        if let Some(w) = found {
            break Some(w.id);
        }
        if cancel.check().is_err() || started.elapsed() >= WINDOW_WAIT {
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut app = desktop.app(pid)?;
    app.windows = desktop.windows(pid)?;
    let mut opened = Opened {
        app,
        new_process,
        new_windows: window.into_iter().collect(),
        restored_windows: Vec::new(),
        front_restored: false,
    };
    let front_now = desktop.user_focus().frontmost_pid;
    if front_now == pid && front_before != pid && front_before != 0 {
        opened.front_restored = desktop.activate(front_before).is_ok();
    }
    // A browser this launch started is reported even when stopped, so it is owned and quit.
    if window.is_none() && !new_process {
        cancel.check()?;
    }
    Ok(opened)
}

/// An app as the user runs it, a browser with no debugging port, say, opened in the background
/// for a test. One that takes the front as it opens a window gets it taken straight back.
/// Returns its pid and its first titled window.
pub fn open_plain<D: Desktop>(
    desktop: &mut D,
    app: &str,
    args: &[String],
) -> CuResult<(i32, Option<u32>)> {
    let before: HashSet<i32> = desktop.apps()?.iter().map(|a| a.pid).collect();
    let front_before = desktop.user_focus().frontmost_pid;
    desktop.open_new(app, args)?;
    let started = Instant::now();
    let (mut pid, mut window) = (None, None);
    while started.elapsed() < WINDOW_WAIT {
        let front = desktop.user_focus().frontmost_pid;
        if front != front_before && front_before != 0 && !before.contains(&front) {
            let _ = desktop.activate(front_before);
        }
        let fresh = desktop.apps()?.into_iter().find(|a| {
            !before.contains(&a.pid)
                && a.bundle_path.as_deref().map(|p| p.trim_end_matches('/'))
                    == Some(app.trim_end_matches('/'))
        });
        if let Some(a) = fresh {
            pid = Some(a.pid);
            window = a.windows.iter().find(|w| !w.title.is_empty()).map(|w| w.id);
            // Watched a moment after the window shows: the app may take the front late.
            if window.is_some() && started.elapsed() > Duration::from_secs(2) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let pid = pid.ok_or_else(|| CuError::new(ErrorCode::NoSuchTarget, "the app didn't start"))?;
    Ok((pid, window))
}
