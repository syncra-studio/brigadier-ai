//! `brigadierd mcp`: the stdio MCP server a CLI session starts for the Brigadier tools.
//!
//! The CLI spawns `brigadierd mcp --data-dir <data dir>` with the session's grant in
//! [`GRANT_ENV`]. The bridge connects to the running daemon's IPC endpoint (derived from the
//! data directory, like the app finds it), sends [`ClientFrame::Mcp`] instead of a hello (it
//! never reads the UI token), then copies bytes both ways until either side closes. The daemon
//! serves MCP on its end of the connection (`upgrade.rs`).
//!
//! No async runtime, no logging setup: two plain threads, so it starts in a few milliseconds.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use brigadier_ipc::protocol::ClientFrame;
use brigadier_sandbox::PlatformOptions;
use interprocess::local_socket::traits::Stream as _;

/// The environment variable holding the session's grant (no KEY/SECRET/TOKEN in the name, which
/// Codex's default environment filter would drop).
pub const GRANT_ENV: &str = "BRIGADIER_MCP_GRANT";

/// Runs the bridge with the arguments after `mcp`.
pub fn run(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let mut data_dir: Option<PathBuf> = None;
    let mut grant_env = GRANT_ENV.to_owned();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--data-dir") => match args.next() {
                Some(dir) => data_dir = Some(dir.into()),
                None => return usage("--data-dir needs a path"),
            },
            // The computer server's grant has its own variable (Claude merges every server's
            // environment into one).
            Some("--grant-env") => match args.next().and_then(|n| n.into_string().ok()) {
                Some(name) if name.starts_with("BRIGADIER_") => grant_env = name,
                _ => return usage("--grant-env needs a BRIGADIER_ variable name"),
            },
            _ => return usage(&format!("unknown argument {arg:?}")),
        }
    }
    let Some(grant) = std::env::var(&grant_env)
        .ok()
        .filter(|grant| !grant.is_empty())
    else {
        eprintln!("brigadierd mcp: {grant_env} is not set");
        return ExitCode::from(2);
    };
    let platform = match brigadier_sandbox::native(PlatformOptions { data_dir }) {
        Ok(platform) => platform,
        Err(err) => {
            eprintln!("brigadierd mcp: {err}");
            return ExitCode::from(1);
        }
    };
    let stream =
        match brigadier_ipc::connect_blocking(platform.paths(), &ClientFrame::Mcp { grant }) {
            Ok(stream) => stream,
            Err(err) => {
                eprintln!("brigadierd mcp: cannot reach brigadierd: {err}");
                return ExitCode::from(1);
            }
        };
    let (mut from_daemon, mut to_daemon) = stream.split();

    // The CLI closing stdin ends the session; closing the socket tells the daemon.
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut to_daemon);
        std::process::exit(0);
    });

    // Each response is flushed as soon as it arrives.
    let mut stdout = std::io::stdout().lock();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut answered = false;
    loop {
        let read = match from_daemon.read(&mut buffer) {
            Ok(0) if !answered => {
                eprintln!("brigadierd mcp: brigadierd refused this session's grant");
                return ExitCode::from(1);
            }
            Ok(0) => break,
            Ok(read) => read,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("brigadierd mcp: connection to brigadierd failed: {err}");
                return ExitCode::from(1);
            }
        };
        answered = true;
        if stdout.write_all(&buffer[..read]).is_err() || stdout.flush().is_err() {
            break;
        }
    }
    // The daemon closed the connection (the session ended, or brigadierd is shutting down).
    ExitCode::SUCCESS
}

fn usage(error: &str) -> ExitCode {
    eprintln!("brigadierd mcp: {error}\nusage: brigadierd mcp [--data-dir PATH]");
    ExitCode::from(2)
}
