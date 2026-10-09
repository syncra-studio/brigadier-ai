//! `brigadierd computer <tool> [<json arguments>]`: the computer tools from a worker's shell
//! (COMPUTER-USE-PLAN.md §4.6).
//!
//!   brigadierd computer apps
//!   brigadierd computer observe '{"window": 812, "screenshot": "always"}'
//!   brigadierd computer act '{"window": 812, "actions": [{"do": "click", "ref": "e7"}]}'
//!
//! It authenticates with the worker's computer grant ([`GRANT_ENV`]) and goes through the
//! daemon exactly like the `computer` MCP server: it speaks MCP over the same connection, so
//! the broker checks the grant, the permission level, leases and the block list the same way.
//! Text goes to stdout; an image is saved under the worker's temporary folder (`$TMPDIR`, its
//! scratch folder, removed with the worker) and its path printed.

use std::ffi::OsString;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use base64::Engine as _;
use brigadier_ipc::protocol::ClientFrame;
use brigadier_sandbox::PlatformOptions;
use interprocess::local_socket::traits::Stream as _;
use serde_json::{Value, json};

/// The worker's computer grant (set in its CLI's environment).
pub const GRANT_ENV: &str = "BRIGADIER_COMPUTER_GRANT";
/// The data directory of the daemon that issued it. Not `BRIGADIER_DATA_DIR`: a dev build the
/// worker starts must not inherit its host's data.
pub const DATA_DIR_ENV: &str = "BRIGADIER_COMPUTER_DATA_DIR";

const USAGE: &str = "usage: brigadierd computer [--data-dir PATH] apps|launch|observe|act|zoom ['<json arguments>']";

pub fn run(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let mut data_dir: Option<PathBuf> = std::env::var_os(DATA_DIR_ENV).map(PathBuf::from);
    let mut rest: Vec<String> = Vec::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--data-dir") => match args.next() {
                Some(dir) => data_dir = Some(dir.into()),
                None => return fail("--data-dir needs a path"),
            },
            Some(other) => rest.push(other.to_owned()),
            None => return fail("arguments must be text"),
        }
    }
    let Some(tool) = rest.first().cloned() else {
        return fail(USAGE);
    };
    if !matches!(
        tool.as_str(),
        "apps" | "launch" | "observe" | "act" | "zoom"
    ) {
        return fail(USAGE);
    }
    let arguments: Value = match rest.get(1) {
        Some(text) => match serde_json::from_str(text) {
            Ok(v) => v,
            Err(err) => return fail(&format!("the arguments aren't JSON: {err}")),
        },
        None => json!({}),
    };
    let Some(grant) = std::env::var(GRANT_ENV).ok().filter(|g| !g.is_empty()) else {
        return fail(&format!(
            "{GRANT_ENV} is not set: only a Brigadier worker's shell can use computer tools"
        ));
    };
    let platform = match brigadier_sandbox::native(PlatformOptions { data_dir }) {
        Ok(platform) => platform,
        Err(err) => return fail(&err.to_string()),
    };
    let stream =
        match brigadier_ipc::connect_blocking(platform.paths(), &ClientFrame::Mcp { grant }) {
            Ok(stream) => stream,
            Err(err) => return fail(&format!("cannot reach brigadierd: {err}")),
        };
    let (reader, mut writer) = stream.split();
    let mut reader = BufReader::new(reader);
    let mut send = |v: Value| -> std::io::Result<()> {
        let mut line = serde_json::to_vec(&v)?;
        line.push(b'\n');
        writer.write_all(&line)?;
        writer.flush()
    };
    let mut answer = |id: u64| -> Result<Value, String> {
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => return Err("brigadierd refused this grant (the worker ended?)".into()),
                Ok(_) => {}
                Err(err) => return Err(err.to_string()),
            }
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(v);
            }
        }
    };
    let handshake = send(json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "brigadierd computer", "version": env!("CARGO_PKG_VERSION")}
        }
    }))
    .map_err(|e| e.to_string())
    .and_then(|()| answer(1));
    if let Err(err) = handshake {
        return fail(&err);
    }
    let called = send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .and_then(|()| {
            send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": {"name": tool, "arguments": arguments}
            }))
        })
        .map_err(|e| e.to_string())
        .and_then(|()| answer(2));
    let reply = match called {
        Ok(v) => v,
        Err(err) => return fail(&err),
    };
    if let Some(err) = reply.get("error") {
        return fail(
            &err["message"]
                .as_str()
                .unwrap_or("the call failed")
                .to_owned(),
        );
    }
    let result = &reply["result"];
    let mut out = String::new();
    for block in result["content"].as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") => {
                out.push_str(block["text"].as_str().unwrap_or_default());
                if !out.ends_with('\n') {
                    out.push('\n');
                }
            }
            Some("image") => match save_image(block) {
                Ok(path) => out.push_str(&format!("[image saved to {}]\n", path.display())),
                Err(err) => out.push_str(&format!("[image not saved: {err}]\n")),
            },
            _ => {}
        }
    }
    print!("{out}");
    if result["isError"].as_bool() == Some(true) {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

/// Saves an image block under the worker's temporary folder, named by the image id when the
/// text names one.
fn save_image(block: &Value) -> Result<PathBuf, String> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(block["data"].as_str().unwrap_or_default())
        .map_err(|e| e.to_string())?;
    let dir = std::env::temp_dir().join("brigadier-computer");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = format!(
        "{}.png",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_micros())
            .unwrap_or(0)
    );
    let path = dir.join(name);
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

fn fail(why: &str) -> ExitCode {
    eprintln!("brigadierd computer: {why}");
    ExitCode::from(2)
}
