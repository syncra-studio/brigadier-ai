//! `brigadier`: the command a Brigadier session calls through its own shell to read and drive
//! Brigadier, like `cmux`'s CLI. It costs a session no context until it calls it.
//!
//! - [`commands`]: the one table of subcommands, which both the parser and the help come from.
//! - [`protocol`]: the call and reply the daemon answers, shared with its handler.
//! - [`client`]: the daemon's socket, reached as the calling session (its id and grant from
//!   the environment).
//! - [`schema`]: the check of each `--json` output against its checked-in schema.
//!
//! No argument or output names a path inside Brigadier's data folder, and the subcommand names
//! are stable: scripts and a session's instructions call them.

pub mod args;
pub mod client;
pub mod commands;
pub mod protocol;
pub mod render;
pub mod schema;

use std::io::Write;

use serde::de::DeserializeOwned;

use args::Invocation;
use protocol::{CliError, CliReply, ErrorCode, ErrorOutput};

/// Runs `brigadier` with the words after its name; returns the exit code.
pub fn run(words: Vec<String>, stdout: &mut dyn Write, stderr: &mut dyn Write) -> u8 {
    let json = words
        .iter()
        .take_while(|word| *word != "--")
        .any(|word| word == "--json");
    let (command, args, json) = match args::parse(words) {
        Ok(Invocation::Help(None)) => {
            let _ = write!(stdout, "{}", commands::help());
            return 0;
        }
        Ok(Invocation::Help(Some(command))) => {
            let _ = write!(stdout, "{}", commands::command_help(command));
            return 0;
        }
        Ok(Invocation::Version) => {
            let _ = writeln!(stdout, "brigadier {}", env!("CARGO_PKG_VERSION"));
            return 0;
        }
        Ok(Invocation::Run {
            command,
            args,
            json,
        }) => (command, args, json),
        Err(err) => {
            let message = format!("{}\nusage: {}", err.message, err.usage);
            return fail(ErrorCode::Usage, &message, json, stdout, stderr);
        }
    };
    let request = match args::request(command, &args) {
        Ok(Some(request)) => request,
        Ok(None) => {
            let message = format!(
                "`brigadier {}` is not available yet in this version of Brigadier",
                command.name
            );
            return fail(ErrorCode::NotAvailable, &message, json, stdout, stderr);
        }
        Err(err) => {
            let message = format!("{}\nusage: {}", err.message, err.usage);
            return fail(ErrorCode::Usage, &message, json, stdout, stderr);
        }
    };
    let reply = match (client::Caller::from_env(), client::daemon_paths()) {
        (Err(refused), _) => refused,
        (_, Err(unreachable)) => unreachable,
        (Ok(caller), Ok(paths)) => client::call(&paths, &caller, &client::wrap(request)),
    };
    let output = match reply {
        CliReply::Ok { output } => output,
        CliReply::Error {
            error: CliError { code, message },
        } => return fail(code, &message, json, stdout, stderr),
    };
    let rendered = match command.name {
        "sessions" => present(output, json, render::sessions),
        "workers" => present(output, json, render::workers),
        "show" => present(output, json, render::show),
        "read" => present(output, json, render::read),
        other => Err(format!("no output for {other}")),
    };
    match rendered {
        Ok(text) => {
            let _ = write!(stdout, "{text}");
            0
        }
        Err(err) => fail(
            ErrorCode::Failed,
            &format!("Brigadier answered something this brigadier can't read: {err}"),
            json,
            stdout,
            stderr,
        ),
    }
}

/// The output as JSON (decoded first, so `--json` only ever prints the documented shape) or
/// as text.
fn present<T>(
    output: serde_json::Value,
    json: bool,
    text: fn(&T) -> String,
) -> Result<String, String>
where
    T: DeserializeOwned + serde::Serialize,
{
    let typed: T = serde_json::from_value(output).map_err(|err| err.to_string())?;
    if json {
        serde_json::to_string_pretty(&typed)
            .map(|json| json + "\n")
            .map_err(|err| err.to_string())
    } else {
        Ok(text(&typed))
    }
}

fn fail(
    code: ErrorCode,
    message: &str,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    if json {
        let output = ErrorOutput {
            error: CliError {
                code,
                message: message.to_owned(),
            },
        };
        if let Ok(text) = serde_json::to_string_pretty(&output) {
            let _ = writeln!(stdout, "{text}");
        }
    }
    let _ = writeln!(stderr, "brigadier: {message}");
    code.exit_code()
}
