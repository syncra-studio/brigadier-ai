//! Daemon glue for the activity store (brigadier-next plans/brigadier-next/computer-use.md
//! sections 4 and 6a): opens the per-profile [`ActivityHost`], derives the
//! caller identity of each connection, gates and records tool calls, captures
//! frames, and serves the `activity_*` socket methods.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::activity::host::{
    outcome_from_result, ActivityHost, ActivityUpdate, CallStart, CapturedFrames, Gate, UserOp,
};
use crate::activity::model::{
    AgentClass, AgentIdentity, Attribution, CallKind, Caller, ClickPoint, Origin, Profile,
    RecordingMode, SessionId, Target,
};
use crate::activity::redact::RedactPolicy;
use cua_driver_core::recording::now_ms;
use serde_json::{json, Value};

use crate::serve::{DaemonProfile, DaemonResponse};

/// Env override for the store directory (tests, embedding hosts).
pub const ACTIVITY_DIR_ENV: &str = "BRIGADIER_CU_ACTIVITY_DIR";
/// Disables the activity store (kill switch).
pub const ACTIVITY_DISABLED_ENV: &str = "BRIGADIER_CU_ACTIVITY_DISABLED";

const IDLE_MS: u64 = 5 * 60 * 1000;
const END_TTL_MS: u64 = 30 * 60 * 1000;
const FRAME_CAPTURE_BUDGET_MS: u64 = 300;

/// Opens the activity host for this daemon, or `None` when disabled or the
/// store cannot open (the daemon keeps working without it).
pub fn open(profile: DaemonProfile, socket_path: &str) -> Option<Arc<ActivityHost>> {
    if crate::bundle::is_env_truthy(ACTIVITY_DISABLED_ENV) {
        return None;
    }
    let profile = match profile {
        DaemonProfile::Native => Profile::Native,
    };
    let root = store_root(profile, socket_path);
    match ActivityHost::open(profile, "local", &root, now_ms()) {
        Ok(host) => {
            let host = Arc::new(host);
            spawn_idle_sweep(host.clone());
            Some(host)
        }
        Err(error) => {
            tracing::warn!(%error, path = %root.display(), "activity store unavailable");
            None
        }
    }
}

fn store_root(profile: Profile, socket_path: &str) -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os(ACTIVITY_DIR_ENV).filter(|v| !v.is_empty()) {
        return std::path::PathBuf::from(dir);
    }
    let base = crate::home::home().join("activity");
    // Tag scopes run separate daemons on separate sockets: one store each.
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    socket_path.hash(&mut hasher);
    let tag = match profile {
        Profile::Native => "native",
        _ => "other",
    };
    base.join(format!("activity-{tag}-{:08x}", hasher.finish() as u32))
}

fn spawn_idle_sweep(host: Arc<ActivityHost>) {
    // Same cadence class as the daemon's existing session idle sweep.
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tick.tick().await;
            host.sweep_idle(IDLE_MS, END_TTL_MS, now_ms());
        }
    });
}

/// Identity of one socket connection. `user` is true when the request is a
/// host-authorized one (the embedding Brigadier app); otherwise the caller is an
/// agent keyed by the process that runs its MCP proxy.
pub fn caller(peer_pid: Option<u32>, user: bool) -> Caller {
    if user {
        return Caller {
            identity: identity("user", "Brigadier", AgentClass::User, None, None),
            origin: Origin::User,
        };
    }
    let Some(pid) = peer_pid else {
        return Caller {
            identity: identity("anonymous", "unknown", AgentClass::Agent, None, None),
            origin: Origin::Mcp,
        };
    };
    let (proxy_start, agent_pid) = process_facts(pid);
    // The agent is the proxy's parent (Claude Code, Codex, an ACP harness);
    // keying on it keeps one actor across short-lived proxy generations.
    let agent = agent_pid.filter(|p| *p > 1);
    let kind = agent
        .and_then(|p| resolve_process_name(i64::from(p)))
        .map(|name| agent_kind(&name))
        .unwrap_or_else(|| "unknown".to_owned());
    let actor = match agent {
        Some(p) => format!("agent:{p}"),
        None => format!("proc:{pid}"),
    };
    Caller {
        identity: identity(&actor, &kind, AgentClass::Agent, Some(pid), proxy_start),
        origin: Origin::Mcp,
    }
}

fn identity(
    actor: &str,
    kind: &str,
    class: AgentClass,
    pid: Option<u32>,
    start: Option<i64>,
) -> AgentIdentity {
    AgentIdentity {
        attribution: Attribution::None,
        kind: kind.to_owned(),
        class,
        actor: actor.to_owned(),
        on_behalf_of: None,
        agent_id: None,
        terminal_id: None,
        acp_session: None,
        workspace_id: None,
        harness_session_id: None,
        proxy_pid: pid,
        proxy_pid_start: start,
    }
}

/// The executable name of `pid` (`ps -o comm=`), without its directory.
fn resolve_process_name(pid: i64) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    #[cfg(unix)]
    {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "comm="])
            .output()
            .ok()?;
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if raw.is_empty() {
            return None;
        }
        Some(
            std::path::Path::new(&raw)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&raw)
                .to_owned(),
        )
    }
}

fn agent_kind(process_name: &str) -> String {
    let lower = process_name.to_ascii_lowercase();
    if lower.contains("claude") {
        "claude".into()
    } else if lower.contains("codex") {
        "codex".into()
    } else if lower.contains("opencode") {
        "opencode".into()
    } else if lower == "node" || lower == "bun" {
        "node".into()
    } else {
        lower
    }
}

/// (start time, parent pid) of a process, cached per pid generation.
fn process_facts(pid: u32) -> (Option<i64>, Option<u32>) {
    static CACHE: OnceLock<Mutex<HashMap<u32, (Option<i64>, Option<u32>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&pid).copied()) {
        return hit;
    }
    let facts = read_process_facts(pid);
    if let Ok(mut c) = cache.lock() {
        if c.len() > 4096 {
            c.clear();
        }
        c.insert(pid, facts);
    }
    facts
}

#[cfg(not(target_os = "linux"))]
fn read_process_facts(pid: u32) -> (Option<i64>, Option<u32>) {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "ppid=,lstart=", "-p", &pid.to_string()])
        .output()
    else {
        return (None, None);
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut parts = text.split_whitespace();
    let ppid = parts.next().and_then(|v| v.parse().ok());
    // lstart is a date string; hash it into a stable generation marker.
    let rest: Vec<&str> = parts.collect();
    let start = (!rest.is_empty()).then(|| {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        rest.hash(&mut hasher);
        (hasher.finish() >> 1) as i64
    });
    (start, ppid)
}

/// The label a call's session is keyed by.
pub fn label(explicit: Option<&str>, minted: Option<&str>) -> String {
    explicit
        .or(minted)
        .filter(|s| !s.is_empty())
        .unwrap_or("anonymous")
        .to_owned()
}

pub fn target_of(args: &Value) -> Option<Target> {
    let pid = args.get("pid").and_then(Value::as_i64);
    let window_id = args.get("window_id").and_then(Value::as_u64);
    let app_name = args.get("app").and_then(Value::as_str).map(str::to_owned);
    if pid.is_none() && window_id.is_none() && app_name.is_none() {
        return None;
    }
    Some(Target {
        pid,
        window_id,
        app_name,
        ..Target::default()
    })
}

/// Gate a tool call. `args` are the call's arguments after the daemon's own
/// injections; the idempotency key is the reserved `_idempotency_key`.
pub fn begin(host: &ActivityHost, caller: &Caller, label: &str, tool: &str, args: &Value) -> Gate {
    let key = args
        .get("_idempotency_key")
        .and_then(Value::as_str)
        .map(str::to_owned);
    host.begin(caller, label, tool, args, target_of(args), key, now_ms())
}

/// The daemon reply for a refused call (a tool error the agent can read).
pub fn refusal(code: &str, message: &str) -> DaemonResponse {
    DaemonResponse::ok(json!({
        "content": [{"type": "text", "text": format!("Computer use refused: {message} ({code}).")}],
        "isError": true,
        "structuredContent": {"error": code, "message": message},
    }))
}

/// Records a finished call: outcome from the tool result, an after frame
/// from the result's own image (observe) or a fresh window capture (act).
pub async fn finish(host: Arc<ActivityHost>, start: CallStart, args: &Value, result: &Value) {
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut outcome = outcome_from_result(is_error, result.get("structuredContent"));
    outcome.reply = Value::Null;
    let mut frames = CapturedFrames::default();
    if let (Some(x), Some(y)) = (
        args.get("x").and_then(Value::as_f64),
        args.get("y").and_then(Value::as_f64),
    ) {
        frames.click_point = Some(ClickPoint { x, y });
    }
    let image_in_result = result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .find(|c| c.get("type").and_then(Value::as_str) == Some("image"))
        })
        .and_then(|c| c.get("data").and_then(Value::as_str))
        .and_then(|data| {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.decode(data).ok()
        });
    frames.after_png = match (image_in_result, start.kind) {
        (Some(bytes), _) => Some(bytes),
        (None, CallKind::Act) => {
            let window_id = args.get("window_id").and_then(Value::as_u64);
            let pid = args.get("pid").and_then(Value::as_i64);
            if window_id.is_some() || pid.is_some() {
                let capture = tokio::task::spawn_blocking(move || {
                    cua_driver_core::recording::screenshot_for(window_id, pid)
                });
                tokio::time::timeout(
                    std::time::Duration::from_millis(FRAME_CAPTURE_BUDGET_MS),
                    capture,
                )
                .await
                .ok()
                .and_then(Result::ok)
                .flatten()
            } else {
                None
            }
        }
        (None, CallKind::Observe) => None,
    };
    let _ =
        tokio::task::spawn_blocking(move || host.finish(start, outcome, frames, now_ms())).await;
}

/// Whether `method` is an activity method this module serves.
pub fn is_activity_method(method: &str) -> bool {
    method.starts_with("activity_")
}

/// Serves one non-streaming `activity_*` request.
pub fn handle(host: &ActivityHost, method: &str, args: &Value, caller: &Caller) -> DaemonResponse {
    let id = || {
        args.get("id")
            .and_then(Value::as_str)
            .map(|s| SessionId(s.to_owned()))
    };
    let user_op = |op: UserOp| match host.user_op(caller, op, now_ms()) {
        Ok(applied) => DaemonResponse::ok(json!({"applied": applied})),
        Err(reject) => DaemonResponse::err(
            reject.code(),
            if reject.code() == "user_required" {
                77
            } else {
                1
            },
        ),
    };
    match method {
        "activity_sessions_list" => {
            let live = match args.get("status").and_then(Value::as_str) {
                Some("live") => Some(true),
                Some("ended") => Some(false),
                _ => None,
            };
            let since = args.get("since_ms").and_then(Value::as_u64);
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(500) as usize;
            DaemonResponse::ok(json!({
                "profile": host.profile(),
                "machine": host.machine(),
                "sessions": host.sessions(live, since, limit),
            }))
        }
        "activity_session_get" => match id().and_then(|id| host.session(&id)) {
            Some(record) => DaemonResponse::ok(json!(record)),
            None => DaemonResponse::err("unknown_session".to_owned(), 1),
        },
        "activity_timeline" => {
            let Some(id) = id() else {
                return DaemonResponse::err("missing id".to_owned(), 64);
            };
            let after = args.get("after_seq").and_then(Value::as_u64);
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(500)
                .min(5000) as usize;
            match host.timeline(&id, after, limit) {
                Ok(page) => DaemonResponse::ok(json!(page)),
                Err(error) => DaemonResponse::err(error.to_string(), 1),
            }
        }
        "activity_frame" => {
            let Some(blob) = args.get("blob").and_then(Value::as_str) else {
                return DaemonResponse::err("missing blob".to_owned(), 64);
            };
            let full = args.get("size").and_then(Value::as_str) == Some("full");
            let session = id();
            match host.frame(session.as_ref(), blob, full) {
                Ok((mime, bytes)) => {
                    use base64::Engine as _;
                    DaemonResponse::ok(json!({
                        "mime": mime,
                        "data_base64": base64::engine::general_purpose::STANDARD.encode(bytes),
                    }))
                }
                Err(error) => DaemonResponse::err(error.to_string(), 1),
            }
        }
        "activity_session_stop" | "activity_session_pause" | "activity_session_resume" => {
            let Some(id) = id() else {
                return DaemonResponse::err("missing id".to_owned(), 64);
            };
            user_op(match method {
                "activity_session_stop" => UserOp::Stop(id),
                "activity_session_pause" => UserOp::Pause(id),
                _ => UserOp::Resume(id),
            })
        }
        "activity_agent_stop" | "activity_agent_allow" => {
            let Some(actor) = args.get("actor").and_then(Value::as_str).map(str::to_owned) else {
                return DaemonResponse::err("missing actor".to_owned(), 64);
            };
            user_op(if method == "activity_agent_stop" {
                UserOp::StopAgent(actor)
            } else {
                UserOp::AllowAgent(actor)
            })
        }
        "activity_recording_set" => {
            let Some(id) = id() else {
                return DaemonResponse::err("missing id".to_owned(), 64);
            };
            let mode = match args.get("mode").and_then(Value::as_str) {
                Some("events") => RecordingMode::Events,
                Some("events+frames") => RecordingMode::EventsFrames,
                Some("video") => RecordingMode::Video,
                _ => {
                    return DaemonResponse::err(
                        "mode must be events, events+frames or video".to_owned(),
                        64,
                    )
                }
            };
            user_op(UserOp::SetRecording(id, mode))
        }
        "activity_policy_get" => DaemonResponse::ok(json!(host.redact_policy())),
        "activity_policy_set" => {
            let policy = RedactPolicy {
                store_javascript: args
                    .get("store_javascript")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            };
            match host.set_redact_policy(caller, policy) {
                Ok(()) => DaemonResponse::ok(json!(policy)),
                Err(reject) => DaemonResponse::err(reject.code(), 77),
            }
        }
        _ => DaemonResponse::err(format!("unknown activity method {method}"), 64),
    }
}

/// One line of an `activity_subscribe` stream, or `None` when the update
/// is not for this subscriber.
pub fn stream_line(
    update: &ActivityUpdate,
    sessions: bool,
    events_for: &[String],
) -> Option<String> {
    let wanted = match update {
        ActivityUpdate::Sessions { .. } => sessions,
        ActivityUpdate::Events { session, .. } => {
            events_for.is_empty() || events_for.iter().any(|id| id == session.as_str())
        }
    };
    wanted.then(|| {
        serde_json::to_string(&DaemonResponse::ok(json!(update))).unwrap_or_default() + "\n"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_targets() {
        assert_eq!(label(Some("research"), Some("m1")), "research");
        assert_eq!(label(None, Some("m1")), "m1");
        assert_eq!(label(Some(""), None), "anonymous");
        assert!(target_of(&json!({"x": 1})).is_none());
        assert_eq!(
            target_of(&json!({"pid": 3, "window_id": 9}))
                .unwrap()
                .window_id,
            Some(9)
        );
    }

    #[test]
    fn user_connections_are_users_and_agents_are_keyed_by_their_parent() {
        assert!(caller(None, true).is_user());
        let me = caller(Some(std::process::id()), false);
        assert!(!me.is_user());
        assert!(me.identity.actor.starts_with("agent:") || me.identity.actor.starts_with("proc:"));
        assert_eq!(caller(None, false).identity.actor, "anonymous");
    }

    #[test]
    fn activity_methods_round_trip_through_the_handler() {
        let dir = tempfile::tempdir().unwrap();
        let host = ActivityHost::open(Profile::Native, "local", dir.path(), now_ms()).unwrap();
        let agent = caller(Some(std::process::id()), false);
        let user = caller(None, true);
        let Gate::Run(start) = begin(&host, &agent, "demo", "click", &json!({"x": 4, "y": 5}))
        else {
            panic!()
        };
        let id = start.session.as_str().to_owned();
        host.finish(
            start,
            outcome_from_result(false, None),
            CapturedFrames::default(),
            now_ms(),
        );
        let list = handle(&host, "activity_sessions_list", &json!({}), &agent);
        assert!(list.ok);
        assert_eq!(list.result.unwrap()["sessions"][0]["id"], json!(id));
        let page = handle(&host, "activity_timeline", &json!({"id": id}), &agent)
            .result
            .unwrap();
        assert_eq!(page["events"].as_array().unwrap().len(), 2);
        let denied = handle(&host, "activity_session_stop", &json!({"id": id}), &agent);
        assert!(!denied.ok);
        assert_eq!(denied.exit_code, Some(77));
        let stopped = handle(&host, "activity_session_stop", &json!({"id": id}), &user);
        assert_eq!(stopped.result.unwrap()["applied"], json!(true));
        assert!(matches!(
            begin(&host, &agent, "demo", "click", &json!({})),
            Gate::Refuse { .. }
        ));
        assert!(
            !handle(
                &host,
                "activity_policy_set",
                &json!({"store_javascript": true}),
                &agent
            )
            .ok
        );
        assert!(
            handle(
                &host,
                "activity_policy_set",
                &json!({"store_javascript": true}),
                &user
            )
            .ok
        );
    }
}
