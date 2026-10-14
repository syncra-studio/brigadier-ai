//! `brigadier-cu host …`: what Brigadier's own daemon asks of the helper.
//!
//! The helper's socket only takes peers signed like the helper itself, so
//! Brigadier runs this binary for each request instead of connecting itself.
//! Every subcommand prints one JSON line and exits 0, or prints
//! `{"error": …}` and exits 1.
//!
//! - `probe`: `{"state": "healthy"|"absent"|"hung", "current", "build_id",
//!   "pid"}`. Never starts anything.
//! - `status`: `probe`, plus `permissions` (`permissions_status`) and
//!   `sessions` (`agent_sessions`) when the daemon is healthy. Never starts
//!   anything.
//! - `ensure`: starts or replaces the daemon as a shim would.
//! - `restart`: stops and starts the daemon (a hung one).
//! - `stop`, `off`, `on`: stop it; turn computer use off (and stop it) or
//!   back on. Under the lifecycle lock, so no shim starts one in between.
//! - `request <method> [<json args>] [--name <name>]`: one daemon request,
//!   only if a daemon is already running.

use crate::serve::{default_socket_path, send_request, DaemonRequest};
use crate::supervisor::{self, Health};

/// Runs `brigadier-cu host <subcommand> <args…>` and exits.
pub fn run(subcommand: &str, args: &[String]) -> ! {
    let socket = default_socket_path();
    let result = match subcommand {
        "probe" => Ok(probe(&socket)),
        "status" => Ok(status(&socket)),
        "ensure" => supervisor::ensure(&socket, false).map(|()| probe(&socket)),
        "restart" => supervisor::restart(&socket).map(|()| probe(&socket)),
        "stop" => supervisor::stop(&socket).map(|()| serde_json::json!({"stopped": true})),
        "off" => supervisor::set_off(&socket, true).map(|()| serde_json::json!({"off": true})),
        "on" => supervisor::set_off(&socket, false).map(|()| serde_json::json!({"off": false})),
        "request" => request(&socket, args),
        other => Err(anyhow::anyhow!("unknown host subcommand `{other}`")),
    };
    match result {
        Ok(value) => {
            println!("{value}");
            std::process::exit(0);
        }
        Err(error) => {
            println!("{}", serde_json::json!({"error": format!("{error:#}")}));
            std::process::exit(1);
        }
    }
}

fn probe(socket: &str) -> serde_json::Value {
    let health = supervisor::probe(socket);
    let (state, build_id) = match &health {
        Health::Healthy { build_id } => ("healthy", build_id.clone()),
        Health::Absent => ("absent", None),
        Health::Hung => ("hung", None),
        Health::Refused => ("refused", None),
    };
    serde_json::json!({
        "state": state,
        "current": health.is_healthy() && supervisor::is_current(socket),
        "build_id": build_id,
        "pid": supervisor::running_daemon(socket),
    })
}

fn status(socket: &str) -> serde_json::Value {
    let mut value = probe(socket);
    if value["state"] == "healthy" {
        let ask = |method: &str| {
            send_request(
                socket,
                &DaemonRequest {
                    method: method.to_owned(),
                    name: None,
                    args: None,
                    session_id: None,
                },
            )
            .ok()
            .filter(|response| response.ok)
            .and_then(|response| response.result)
        };
        value["permissions"] = ask("permissions_status").unwrap_or_default();
        value["sessions"] = ask("agent_sessions")
            .and_then(|sessions| sessions.get("sessions").cloned())
            .unwrap_or_default();
    }
    value
}

fn request(socket: &str, args: &[String]) -> anyhow::Result<serde_json::Value> {
    let (method, rest) = args
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("usage: host request <method> [<json args>]"))?;
    let mut name = None;
    let mut json = None;
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        if arg == "--name" {
            name = rest.next().cloned();
        } else {
            json = Some(serde_json::from_str(arg)?);
        }
    }
    if !supervisor::probe(socket).is_healthy() {
        anyhow::bail!("the helper isn't running");
    }
    let response = send_request(
        socket,
        &DaemonRequest {
            method: method.clone(),
            name,
            args: json,
            session_id: None,
        },
    )?;
    if response.ok {
        Ok(response.result.unwrap_or(serde_json::Value::Null))
    } else {
        anyhow::bail!(response
            .error
            .unwrap_or_else(|| "the helper refused the request".to_owned()))
    }
}
