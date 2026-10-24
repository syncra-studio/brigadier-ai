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
pub mod models;
pub mod protocol;
pub mod render;
pub mod schema;
pub mod secrets;

use std::io::Write;

use serde::de::DeserializeOwned;

use args::Invocation;
use protocol::{CliError, CliReply, ErrorCode, ErrorOutput};

/// Runs `brigadier` with the words after its name; returns the exit code.
pub fn run(words: Vec<String>, stdout: &mut dyn Write, stderr: &mut dyn Write) -> u8 {
    // An MCP server's secrets work without the app (and without a session).
    if let Some(code) = secrets::run_local(
        &words,
        &secrets::Keychain,
        &mut std::io::stdin(),
        stdout,
        stderr,
    ) {
        return code;
    }
    let ledger = words
        .iter()
        .find(|word| !word.starts_with('-'))
        .is_some_and(|word| word == "ledger");
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
            let message = if ledger {
                err.message
            } else {
                format!("{}\nusage: {}", err.message, err.usage)
            };
            return fail(ErrorCode::Usage, &message, json, stdout, stderr);
        }
    };
    let mut args = args;
    // A long text given as none, or `-`: it comes from standard input.
    let action = args
        .positionals
        .get("action")
        .and_then(|a| a.first())
        .map(String::as_str);
    if let Some((name, before)) = text_from_stdin(command.name, action) {
        let words = args.positionals.entry(name).or_default();
        if words.len() == before || words.get(before..) == Some(&["-".to_owned()][..]) {
            let mut text = String::new();
            if let Err(err) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
                let message = format!("could not read the text from standard input: {err}");
                return fail(ErrorCode::Usage, &message, json, stdout, stderr);
            }
            words.truncate(before);
            words.push(text);
        }
    }
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
            let message = if ledger {
                err.message
            } else {
                format!("{}\nusage: {}", err.message, err.usage)
            };
            return fail(ErrorCode::Usage, &message, json, stdout, stderr);
        }
    };
    let answered = match (client::Caller::from_env(), client::daemon_paths()) {
        (Err(refused), _) => return print(refused, command.name, &args, json, stdout, stderr),
        (_, Err(unreachable)) => {
            return print(unreachable, command.name, &args, json, stdout, stderr);
        }
        (Ok(caller), Ok(paths)) => client::call(&paths, &caller, &client::wrap(request)),
    };
    let code = print(
        answered.reply.clone(),
        command.name,
        &args,
        json,
        stdout,
        stderr,
    );
    // A wait's events count as heard once they are printed.
    if command.name == "wait" && code == 0 && stdout.flush().is_ok() {
        answered.heard();
    }
    code
}

/// Prints `reply` to `command`, as text or as JSON; returns the exit code.
fn print(
    reply: CliReply,
    command: &str,
    args: &args::Parsed,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let output = match reply {
        CliReply::Ok { output } => output,
        CliReply::Error {
            error: CliError { code, message },
        } => return fail(code, &message, json, stdout, stderr),
    };
    let rendered = match command {
        "sessions" => present(output, json, render::sessions),
        "models" => present(output, json, render::models),
        "workers" => present(output, json, render::workers),
        "show" => present(output, json, render::show),
        "read" => present(output, json, render::read),
        "start" => present(output, json, render::start),
        "send" => present(output, json, render::send),
        "wait" => present(output, json, render::wait),
        "close" => present(output, json, render::close),
        "continue-on" => present(output, json, render::continue_on),
        "ask" => present(output, json, render::ask),
        "why" => present(output, json, render::why),
        "ledger" => present(output, json, |output: &protocol::LedgerOutput| {
            output.text.clone()
        }),
        "library" => present(output, json, render::library),
        name if json && protocol::Tool::of_command(name).is_some() => {
            present(output, json, render::tool)
        }
        name if protocol::Tool::of_command(name).is_some() => {
            let out = args.flag("out").map(std::path::PathBuf::from);
            serde_json::from_value::<protocol::ToolOutput>(output)
                .map_err(|err| err.to_string())
                .and_then(|output| render::saved_images(output, out.as_deref()))
                .map(|output| render::tool(&output))
        }
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

/// The positional whose text may come from standard input for `command`'s `action`, and how
/// many of its words come before that text (`page write`'s page id).
fn text_from_stdin(command: &str, action: Option<&str>) -> Option<(&'static str, usize)> {
    match (command, action?) {
        ("page", "write") => Some(("args", 1)),
        ("page", "file-write") => Some(("args", 0)),
        ("report", "save") => Some(("markdown", 0)),
        ("ledger", "add") => Some(("args", 0)),
        ("checkpoint", "set") => Some(("args", 0)),
        ("library", "add") => Some(("args", 0)),
        ("feedback", "offer") => Some(("body", 0)),
        _ => None,
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
