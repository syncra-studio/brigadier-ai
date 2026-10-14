//! Which apps each Brigadier agent is using, and the apps the user stopped
//! it from using.
//!
//! An agent's calls carry its host session (`brigadier-<owner>`, stable for
//! the agent's life) and come from its shim's process. The daemon notes each
//! app a call targets, so Brigadier's menu-bar item can list who drives what.
//!
//! "Stop using <app>" adds the session and app to `<home>/stopped.json`,
//! with the session's shim processes at that moment. Its further calls on
//! that app are refused. The list is read at startup, so a stop survives the
//! helper being replaced. An entry lapses when all those shims have exited:
//! the agent is gone, and a new one may use the app again.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

/// An app a call targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub name: String,
    pub bundle_id: Option<String>,
    pub pid: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct Use {
    name: String,
    bundle_id: Option<String>,
    pid: Option<i64>,
    last_used_ms: u64,
}

#[derive(Debug, Default)]
struct Session {
    shims: BTreeSet<u32>,
    apps: Vec<Use>,
}

/// One stop the user asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    pub session: String,
    #[serde(default)]
    pub bundle_id: Option<String>,
    pub app: String,
    /// The session's shims when it was stopped; the stop holds while any runs.
    pub shims: Vec<u32>,
}

#[derive(Default)]
struct State {
    sessions: BTreeMap<String, Session>,
    stops: Option<Vec<Stop>>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}

fn stops_file() -> PathBuf {
    crate::lifecycle::home().join("stopped.json")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn alive(pid: u32) -> bool {
    libc::pid_t::try_from(pid).is_ok_and(|pid| unsafe { libc::kill(pid, 0) } == 0)
}

/// The host session of a minted proxy session id (`<host>-mcp-<pid>-<n>`),
/// for Brigadier's agents only.
pub fn host_session_of(session_id: &str) -> Option<&str> {
    let (host, _) = session_id.rsplit_once("-mcp-")?;
    host.starts_with("brigadier-").then_some(host)
}

/// Whether `stop` names `app`: by bundle id when both have one, else by name.
fn names(stop: &Stop, app: &App) -> bool {
    match (&stop.bundle_id, &app.bundle_id) {
        (Some(stopped), Some(target)) => stopped == target,
        _ => stop.app.eq_ignore_ascii_case(&app.name),
    }
}

/// The stops still in force, loading them from disk on first use.
fn live_stops<'a>(state: &'a mut State, alive: &dyn Fn(u32) -> bool) -> &'a mut Vec<Stop> {
    let stops = state.stops.get_or_insert_with(|| {
        std::fs::read(stops_file())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    });
    stops.retain(|stop| stop.shims.iter().any(|&pid| alive(pid)));
    stops
}

fn save(stops: &[Stop]) {
    let path = stops_file();
    let partial = path.with_extension(format!("json.{}", std::process::id()));
    if let Ok(bytes) = serde_json::to_vec(stops) {
        if std::fs::write(&partial, bytes).is_ok() {
            let _ = std::fs::rename(&partial, &path);
        }
    }
}

/// What the agent is told when it acts on an app the user stopped.
pub fn stopped_message(app: &str) -> String {
    format!(
        "The user stopped you from using {app}. Don't use {app} again in this task unless the \
         user asks you to."
    )
}

/// Notes a call by `session` from shim `shim` on `app`. Returns the refusal
/// to send instead when the user stopped this session using that app.
pub fn note_call(session: &str, shim: Option<u32>, app: Option<App>) -> Option<String> {
    note_call_with(session, shim, app, &alive, now_ms())
}

fn note_call_with(
    session: &str,
    shim: Option<u32>,
    app: Option<App>,
    alive: &dyn Fn(u32) -> bool,
    now: u64,
) -> Option<String> {
    let mut state = state().lock().ok()?;
    if let Some(app) = &app {
        let stops = live_stops(&mut state, alive);
        if let Some(stop) = stops
            .iter()
            .find(|stop| stop.session == session && names(stop, app))
        {
            return Some(stopped_message(&stop.app));
        }
    }
    let entry = state.sessions.entry(session.to_owned()).or_default();
    entry.shims.extend(shim);
    if let Some(app) = app {
        match entry.apps.iter_mut().find(|used| {
            (used.bundle_id.is_some() && used.bundle_id == app.bundle_id)
                || (used.bundle_id.is_none() && used.name == app.name)
        }) {
            Some(used) => {
                used.last_used_ms = now;
                used.pid = app.pid.or(used.pid);
            }
            None => entry.apps.push(Use {
                name: app.name,
                bundle_id: app.bundle_id,
                pid: app.pid,
                last_used_ms: now,
            }),
        }
    }
    None
}

/// A shim's control connection began (also after the helper was replaced).
pub fn shim_connected(session: &str, shim: Option<u32>) {
    if let Ok(mut state) = state().lock() {
        state
            .sessions
            .entry(session.to_owned())
            .or_default()
            .shims
            .extend(shim);
    }
}

/// The agent ended its computer-use session: it no longer uses any app. Its shims stay noted,
/// so a stop still holds if it starts using apps again.
pub fn session_ended(session: &str) {
    if let Ok(mut state) = state().lock() {
        if let Some(entry) = state.sessions.get_mut(session) {
            entry.apps.clear();
        }
    }
}

/// A shim's control connection ended: its agent is gone.
pub fn shim_gone(shim: Option<u32>) {
    let Some(shim) = shim else { return };
    if let Ok(mut state) = state().lock() {
        state.sessions.retain(|_, session| {
            session.shims.remove(&shim);
            !session.shims.is_empty()
        });
    }
}

/// Stops `session` from using an app; its next call there is refused.
pub fn stop(session: &str, bundle_id: Option<String>, app: String) -> Stop {
    stop_with(session, bundle_id, app, &alive)
}

fn stop_with(
    session: &str,
    bundle_id: Option<String>,
    app: String,
    alive: &dyn Fn(u32) -> bool,
) -> Stop {
    let Ok(mut state) = state().lock() else {
        return Stop {
            session: session.to_owned(),
            bundle_id,
            app,
            shims: Vec::new(),
        };
    };
    let shims: Vec<u32> = state
        .sessions
        .get(session)
        .map(|s| s.shims.iter().copied().collect())
        .unwrap_or_default();
    if let Some(entry) = state.sessions.get_mut(session) {
        entry
            .apps
            .retain(|used| match (&bundle_id, &used.bundle_id) {
                (Some(stopped), Some(used)) => stopped != used,
                _ => !used.name.eq_ignore_ascii_case(&app),
            });
    }
    let stop = Stop {
        session: session.to_owned(),
        bundle_id,
        app,
        shims,
    };
    let stops = live_stops(&mut state, alive);
    stops.retain(|existing| !(existing.session == stop.session && existing.app == stop.app));
    stops.push(stop.clone());
    let snapshot = stops.clone();
    drop(state);
    save(&snapshot);
    stop
}

/// The live sessions and the apps each is using, for Brigadier.
/// An app an agent launched has no pid until a later call names one, and a
/// relaunched app has a new one, so each app is listed with the pid
/// `running_pid` finds for its bundle id when the noted one is gone.
pub fn list(running_pid: &dyn Fn(&str) -> Option<i64>) -> serde_json::Value {
    let Ok(state) = state().lock() else {
        return serde_json::json!({"sessions": []});
    };
    let sessions: Vec<serde_json::Value> = state
        .sessions
        .iter()
        .filter(|(_, session)| !session.apps.is_empty())
        .map(|(id, session)| {
            let apps: Vec<Use> = session
                .apps
                .iter()
                .map(|used| {
                    let noted = used.pid.filter(|&pid| u32::try_from(pid).is_ok_and(alive));
                    Use {
                        pid: noted.or_else(|| used.bundle_id.as_deref().and_then(running_pid)),
                        ..used.clone()
                    }
                })
                .collect();
            serde_json::json!({"session": id, "apps": apps})
        })
        .collect();
    serde_json::json!({"sessions": sessions})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, bundle: &str) -> App {
        App {
            name: name.into(),
            bundle_id: Some(bundle.into()),
            pid: Some(42),
        }
    }

    // The state is process-wide; every test uses its own session names.

    #[test]
    fn a_minted_session_names_its_brigadier_agent() {
        assert_eq!(
            host_session_of("brigadier-task-7-mcp-1234-99"),
            Some("brigadier-task-7")
        );
        assert_eq!(host_session_of("research-1-mcp-1-2"), None);
        assert_eq!(host_session_of("brigadier-task-7"), None);
    }

    #[test]
    fn calls_list_the_apps_a_session_uses_until_its_shim_goes() {
        let all_alive = |_: u32| true;
        let s = "brigadier-list-test";
        assert_eq!(
            note_call_with(
                s,
                Some(7001),
                Some(app("TextEdit", "com.apple.TextEdit")),
                &all_alive,
                10
            ),
            None
        );
        assert_eq!(
            note_call_with(
                s,
                Some(7001),
                Some(app("TextEdit", "com.apple.TextEdit")),
                &all_alive,
                20
            ),
            None
        );
        let listed = list(&|_| None);
        let mine: Vec<_> = listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["session"] == s)
            .collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0]["apps"].as_array().unwrap().len(), 1);
        assert_eq!(mine[0]["apps"][0]["last_used_ms"], 20);
        shim_gone(Some(7001));
        assert!(!list(&|_| None).to_string().contains(s));
    }

    #[test]
    fn a_launched_app_is_listed_with_the_pid_it_runs_as() {
        let all_alive = |_: u32| true;
        let s = "brigadier-launched-pid-test";
        let launched = App {
            pid: None,
            ..app("TextEdit", "com.apple.TextEdit")
        };
        assert_eq!(
            note_call_with(s, Some(7201), Some(launched), &all_alive, 10),
            None
        );
        let running = |bundle_id: &str| (bundle_id == "com.apple.TextEdit").then_some(4321);
        let listed = list(&running);
        let mine = listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["session"] == s)
            .unwrap();
        assert_eq!(mine["apps"][0]["pid"], 4321);
        shim_gone(Some(7201));
    }

    #[test]
    fn an_ended_session_is_no_longer_listed_until_it_uses_an_app_again() {
        let all_alive = |_: u32| true;
        let s = "brigadier-end-test";
        let textedit = || Some(app("TextEdit", "com.apple.TextEdit"));
        assert_eq!(
            note_call_with(s, Some(7101), textedit(), &all_alive, 10),
            None
        );
        assert!(list(&|_| None).to_string().contains(s));
        session_ended(s);
        assert!(!list(&|_| None).to_string().contains(s));
        assert_eq!(
            note_call_with(s, Some(7101), textedit(), &all_alive, 20),
            None
        );
        assert!(list(&|_| None).to_string().contains(s));
        shim_gone(Some(7101));
    }

    #[test]
    fn a_stopped_app_is_refused_to_that_session_only_and_lapses_with_its_shims() {
        let home = std::env::temp_dir().join(format!("cu-agent-apps-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var(crate::home::HOME_ENV, &home);
        let all_alive = |_: u32| true;
        let s = "brigadier-stop-test";
        let other = "brigadier-stop-other";
        note_call_with(
            s,
            Some(8001),
            Some(app("TextEdit", "com.apple.TextEdit")),
            &all_alive,
            1,
        );
        let stop = stop_with(
            s,
            Some("com.apple.TextEdit".into()),
            "TextEdit".into(),
            &all_alive,
        );
        assert_eq!(stop.shims, vec![8001]);
        let refused = note_call_with(
            s,
            Some(8001),
            Some(app("TextEdit", "com.apple.TextEdit")),
            &all_alive,
            2,
        );
        assert_eq!(refused, Some(stopped_message("TextEdit")));
        // Another app, and another agent on the same app, still run.
        assert_eq!(
            note_call_with(
                s,
                Some(8001),
                Some(app("Finder", "com.apple.finder")),
                &all_alive,
                3
            ),
            None
        );
        assert_eq!(
            note_call_with(
                other,
                Some(8002),
                Some(app("TextEdit", "com.apple.TextEdit")),
                &all_alive,
                3
            ),
            None
        );
        // The stop is on disk for a replacement helper.
        let saved: Vec<Stop> =
            serde_json::from_slice(&std::fs::read(home.join("stopped.json")).unwrap()).unwrap();
        assert!(saved
            .iter()
            .any(|saved| saved.session == s && saved.shims == vec![8001]));
        // Once the agent's shim has exited, the stop lapses.
        let only_others = |pid: u32| pid != 8001;
        assert_eq!(
            note_call_with(
                s,
                Some(8003),
                Some(app("TextEdit", "com.apple.TextEdit")),
                &only_others,
                4
            ),
            None
        );
        std::fs::remove_dir_all(&home).unwrap();
    }
}
