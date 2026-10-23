//! Parses a command line by the table in [`crate::commands`].

use std::collections::BTreeMap;

use crate::commands::{self, Arg, Command};
use serde_json::{Map, Value, json};

use crate::protocol::{Agent, Answer, Capability, CliRequest, Permission, StartRequest, Tool};

/// What the command line asks for.
#[derive(Debug)]
pub enum Invocation {
    /// `brigadier --help`, or one command's help.
    Help(Option<&'static Command>),
    Version,
    Run {
        command: &'static Command,
        args: Parsed,
        json: bool,
    },
}

/// A command's arguments, checked against its table entry.
#[derive(Debug, Default)]
pub struct Parsed {
    /// Each positional's words, by name.
    pub positionals: BTreeMap<&'static str, Vec<String>>,
    /// Each flag's values (a switch: one empty value), by long name.
    pub flags: BTreeMap<&'static str, Vec<String>>,
}

impl Parsed {
    fn one(&self, name: &str) -> Option<&str> {
        self.positionals
            .get(name)
            .and_then(|words| words.first())
            .map(String::as_str)
    }

    pub fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .get(name)
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    fn switch(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }

    /// Every value of a repeatable flag, in order.
    fn all(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
    }

    /// A positional's words, joined by spaces; empty when none was given.
    fn text(&self, name: &str) -> String {
        self.positionals
            .get(name)
            .map(|words| words.join(" "))
            .unwrap_or_default()
    }

    fn words(&self, name: &str) -> Vec<String> {
        self.positionals.get(name).cloned().unwrap_or_default()
    }
}

/// A command line the table doesn't accept, with the usage line to show.
#[derive(Debug, PartialEq, Eq)]
pub struct UsageError {
    pub message: String,
    pub usage: String,
}

/// Parses the words after `brigadier`.
pub fn parse<I, S>(words: I) -> Result<Invocation, UsageError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut words = words.into_iter().map(Into::into);
    let Some(first) = words.next() else {
        return Ok(Invocation::Help(None));
    };
    let name = match first.as_str() {
        "-h" | "--help" => return Ok(Invocation::Help(None)),
        "-V" | "--version" => return Ok(Invocation::Version),
        "help" => {
            return match words.next() {
                None => Ok(Invocation::Help(None)),
                Some(name) => match commands::find(&name) {
                    Some(command) => Ok(Invocation::Help(Some(command))),
                    None => Err(unknown_command(&name)),
                },
            };
        }
        name => name,
    };
    let Some(command) = commands::find(name) else {
        return Err(unknown_command(name));
    };
    let error = |message: String| UsageError {
        message,
        usage: command.usage(),
    };

    let mut json = false;
    let mut words_left: Vec<String> = Vec::new();
    let mut flags: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut only_words = false;
    while let Some(word) = words.next() {
        if only_words || !word.starts_with("--") && word != "-h" {
            words_left.push(word);
            continue;
        }
        if word == "--" {
            only_words = true;
            continue;
        }
        if word == "--help" || word == "-h" {
            return Ok(Invocation::Help(Some(command)));
        }
        if word == "--json" {
            json = true;
            continue;
        }
        let (long, inline) = match word[2..].split_once('=') {
            Some((long, value)) => (long, Some(value.to_owned())),
            None => (&word[2..], None),
        };
        let Some(Arg::Flag {
            long,
            value,
            choices,
            repeat,
            ..
        }) = command
            .args
            .iter()
            .find(|arg| matches!(arg, Arg::Flag { long: l, .. } if *l == long))
        else {
            return Err(error(format!("unknown option --{long}")));
        };
        let given = match value {
            Some(value_name) => match inline.or_else(|| words.next()) {
                Some(given) => given,
                None => return Err(error(format!("--{long} needs a {value_name}"))),
            },
            None if inline.is_some() => {
                return Err(error(format!("--{long} takes no value")));
            }
            None => String::new(),
        };
        if !choices.is_empty() && !choices.contains(&given.as_str()) {
            return Err(error(format!(
                "--{long} must be one of: {}",
                choices.join(", ")
            )));
        }
        let values = flags.entry(long).or_default();
        if !values.is_empty() && !repeat {
            return Err(error(format!("--{long} is given twice")));
        }
        values.push(given);
    }

    let mut positionals: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
    let mut words_left = words_left.into_iter();
    for arg in command.args {
        let Arg::Positional {
            name,
            required,
            rest,
            choices,
            ..
        } = arg
        else {
            continue;
        };
        let taken: Vec<String> = if *rest {
            words_left.by_ref().collect()
        } else {
            words_left.next().into_iter().collect()
        };
        if taken.is_empty() {
            if *required {
                return Err(error(format!("missing <{name}>")));
            }
            continue;
        }
        if !choices.is_empty()
            && let Some(bad) = taken.iter().find(|word| !choices.contains(&word.as_str()))
        {
            return Err(error(format!(
                "{bad:?} is not one of: {}",
                choices.join(", ")
            )));
        }
        positionals.insert(name, taken);
    }
    if let Some(extra) = words_left.next() {
        return Err(error(format!("unexpected argument {extra:?}")));
    }
    Ok(Invocation::Run {
        command,
        args: Parsed { positionals, flags },
        json,
    })
}

fn unknown_command(name: &str) -> UsageError {
    UsageError {
        message: format!("unknown command {name:?}; `brigadier --help` lists them"),
        usage: "brigadier <command> [arguments] [--json]".into(),
    }
}

/// The request a built command sends; `Ok(None)` for one not built yet.
pub fn request(command: &Command, args: &Parsed) -> Result<Option<CliRequest>, UsageError> {
    let error = |message: String| UsageError {
        message,
        usage: command.usage(),
    };
    let id = || args.one("id").unwrap_or_default().to_owned();
    // Numbers are checked for every command, built or not.
    let number = |name: &str| -> Result<Option<u32>, UsageError> {
        match args.flag(name) {
            None => Ok(None),
            Some(text) => match text.parse::<u32>() {
                Ok(n) if n > 0 => Ok(Some(n)),
                _ => Err(error(format!("--{name} must be a whole number above 0"))),
            },
        }
    };
    for arg in command.args {
        if let Arg::Flag {
            long,
            value: Some(value),
            ..
        } = arg
            && (*value == "N" || *value == "SECONDS")
        {
            number(long)?;
        }
    }
    if !command.available {
        return Ok(None);
    }
    Ok(Some(match command.name {
        "sessions" => CliRequest::Sessions,
        "models" => CliRequest::Models,
        "workers" => CliRequest::Workers,
        "show" => CliRequest::Show { id: id() },
        "read" => {
            let last = number("last")?;
            let screen = args.switch("screen");
            if last.is_some() && screen {
                return Err(error("--last and --screen don't go together".into()));
            }
            CliRequest::Read {
                id: id(),
                last,
                screen,
            }
        }
        "start" => {
            if args.switch("local") && args.switch("worktree") {
                return Err(error("--local and --worktree don't go together".into()));
            }
            if args.switch("local") && args.flag("base").is_some() {
                return Err(error(
                    "--base is for a worktree; --local works on this session's branch".into(),
                ));
            }
            // Sent as an absolute path: the daemon doesn't know the caller's folder.
            let repo = match args
                .flag("repo")
                .map(str::trim)
                .filter(|repo| !repo.is_empty())
            {
                Some(repo) => Some(
                    std::path::absolute(repo)
                        .map_err(|err| error(format!("--repo {repo}: {err}")))?
                        .display()
                        .to_string(),
                ),
                None => None,
            };
            let skills = args.all("skill");
            if !skills.is_empty() && !args.switch("lean") {
                return Err(error(
                    "--skill goes with --lean; a worker without it has your skills".into(),
                ));
            }
            let with: Vec<Capability> = args
                .all("with")
                .iter()
                .filter_map(|word| {
                    Capability::ALL
                        .into_iter()
                        .find(|capability| capability.word() == word)
                })
                .collect();
            if !with.is_empty() && !args.switch("lean") {
                return Err(error(
                    "--with goes with --lean; a worker without it has computer use while it is on"
                        .into(),
                ));
            }
            let prompt = args.text("prompt");
            if prompt.trim().is_empty() {
                return Err(error("missing <prompt>".into()));
            }
            let given = |name: &str| {
                args.flag(name)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            };
            CliRequest::Start(Box::new(StartRequest {
                title: given("title"),
                agent: args.flag("agent").map(|agent| match agent {
                    "codex" => Agent::Codex,
                    _ => Agent::Claude,
                }),
                model: given("model"),
                effort: given("effort"),
                work: given("work"),
                local: args.switch("local"),
                repo,
                base: given("base"),
                permission: args.flag("permission").map(|mode| match mode {
                    "ask" => Permission::Ask,
                    "full-access" => Permission::FullAccess,
                    _ => Permission::AcceptEdits,
                }),
                lean: args.switch("lean"),
                skills,
                with,
                prompt,
            }))
        }
        "send" => {
            let text = args.text("text");
            let answer = match (args.switch("allow"), args.switch("deny")) {
                (true, true) => {
                    return Err(error("--allow and --deny don't go together".into()));
                }
                (true, false) => Some(Answer::Allow),
                (false, true) => Some(Answer::Deny),
                (false, false) => None,
            };
            match (answer, text.trim().is_empty()) {
                (Some(_), false) => {
                    return Err(error(
                        "--allow and --deny answer a permission prompt and take no message".into(),
                    ));
                }
                (None, true) => return Err(error("missing <text>".into())),
                _ => {}
            }
            CliRequest::Send {
                id: id(),
                text: answer.is_none().then_some(text),
                answer,
            }
        }
        "wait" => CliRequest::Wait {
            ids: args.words("id"),
            timeout_secs: number("timeout")?,
        },
        "close" => CliRequest::Close { id: id() },
        "ask" => {
            let question = args.text("question");
            if question.trim().is_empty() {
                return Err(error("missing <question>".into()));
            }
            let options: Vec<String> = args
                .all("option")
                .into_iter()
                .map(|option| option.trim().to_owned())
                .filter(|option| !option.is_empty())
                .collect();
            if options.len() == 1 || options.len() > 4 {
                return Err(error(
                    "give 2 to 4 options, or none for a free answer".into(),
                ));
            }
            CliRequest::Ask {
                question,
                options,
                title: args.flag("title").map(str::to_owned),
            }
        }
        name if Tool::of_command(name).is_some() => tool_request(command, args)?,
        other => return Err(error(format!("{other} has no request yet"))),
    }))
}

/// The kept tools' subcommands: the action, with the tool's own arguments.
fn tool_request(command: &Command, args: &Parsed) -> Result<CliRequest, UsageError> {
    let error = |message: &str| UsageError {
        message: message.to_owned(),
        usage: command.usage(),
    };
    let action = args.one("action").unwrap_or_default().to_owned();
    let words = args.words("args");
    let text = words.join(" ");
    let mut out = Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            out.insert(key.to_owned(), value);
        }
    };
    let flag = |name: &str| args.flag(name).map(|value| json!(value));
    let number = |name: &str| {
        args.flag(name)
            .and_then(|value| value.parse::<u32>().ok())
            .map(|n| json!(n))
    };
    // The one word an action takes, when it takes one.
    let one = |what: &str, required: bool| -> Result<Option<Value>, UsageError> {
        match words.as_slice() {
            [] if required => Err(error(&format!("missing <{what}>"))),
            [] => Ok(None),
            [word] => Ok(Some(json!(word))),
            [_, extra, ..] => Err(error(&format!("unexpected argument {extra:?}"))),
        }
    };
    let tool = match command.name {
        "preview" => {
            match action.as_str() {
                "start" => {
                    if text.trim().is_empty() {
                        return Err(error("missing <command>"));
                    }
                    put("command", Some(json!(text)));
                    put("name", flag("name"));
                    put("kind", flag("kind"));
                    put("workdir", flag("workdir"));
                    put("target", flag("target"));
                    put("purpose", flag("purpose"));
                    let mut env = Map::new();
                    for pair in args.all("env") {
                        let Some((name, value)) = pair.split_once('=') else {
                            return Err(error("--env takes NAME=VALUE"));
                        };
                        env.insert(name.to_owned(), json!(value));
                    }
                    put("env", (!env.is_empty()).then_some(Value::Object(env)));
                }
                "stop" => put("id", one("id", false)?),
                _ => {
                    put("id", one("id", false)?);
                    put("tail_lines", number("tail"));
                    put("wait_secs", number("wait"));
                }
            }
            Tool::Preview
        }
        "browser" => {
            if action != "open" {
                let Some(tab) = args.flag("tab") else {
                    return Err(error("--tab names the tab, as `open` printed it"));
                };
                put("tab", Some(json!(tab)));
            }
            let asks = args.flag("asks").map(|asks| json!(asks.replace('-', "_")));
            match action.as_str() {
                "open" => {
                    put("url", one("url", true)?);
                    put("keep", Some(json!(args.switch("keep"))));
                }
                "navigate" => match (one("url", false)?, args.flag("go")) {
                    (Some(_), Some(_)) => return Err(error("a URL or --go, not both")),
                    (None, None) => return Err(error("missing <url> (or --go)")),
                    (url, go) => {
                        put("url", url);
                        put("go", go.map(|go| json!(go)));
                    }
                },
                "click" => {
                    put("target", one("ref", true)?);
                    put("asks", asks);
                }
                "type" => {
                    let Some((target, typed)) = words.split_first() else {
                        return Err(error("missing <ref>"));
                    };
                    put("target", Some(json!(target)));
                    put("text", Some(json!(typed.join(" "))));
                    put("submit", Some(json!(args.switch("submit"))));
                    put("asks", asks);
                }
                _ => {
                    one("nothing", false)?.map_or(Ok(()), |extra| {
                        Err(error(&format!("unexpected argument {extra}")))
                    })?;
                }
            }
            Tool::Browser
        }
        "app" => {
            match action.as_str() {
                "state" => {}
                "read-terminal" => put("terminal", one("terminal", true)?),
                "show-session" => put("id", one("id", true)?),
                "close-tab" => put("tab", one("tab", true)?),
                "terminal-run" => {
                    if text.trim().is_empty() {
                        return Err(error("missing <command>"));
                    }
                    put("command", Some(json!(text)));
                    put("terminal", flag("terminal"));
                    put("show", Some(json!(args.switch("show"))));
                }
                _ => {
                    let Some((what, target)) = words.split_first() else {
                        return Err(error(
                            "missing <what>: file, diff, output, url, worker or page",
                        ));
                    };
                    let target = target.join(" ");
                    let needs = |key: &str| -> Result<Value, UsageError> {
                        match target.trim() {
                            "" => Err(error(&format!("open {what}: missing <{key}>"))),
                            value => Ok(json!(value)),
                        }
                    };
                    let opened = match what.as_str() {
                        "file" => {
                            let mut file = json!({"kind": "file", "path": needs("path")?});
                            if let Some(line) = number("line") {
                                file["line"] = line;
                            }
                            file
                        }
                        "diff" => match target.trim() {
                            "last-turn" | "branch" | "uncommitted" => {
                                json!({"kind": "diff", "scope": target.trim().replace('-', "_")})
                            }
                            _ => {
                                return Err(error("open diff: last-turn, branch or uncommitted"));
                            }
                        },
                        "output" => json!({"kind": "output", "id": needs("id")?}),
                        "url" => json!({"kind": "url", "url": needs("url")?}),
                        "worker" => json!({"kind": "worker", "task": needs("id")?}),
                        "page" => json!({"kind": "page", "id": needs("id")?}),
                        other => {
                            return Err(error(&format!(
                                "open: {other:?} is not one of: file, diff, output, url, \
                                 worker, page"
                            )));
                        }
                    };
                    put("target", Some(opened));
                    put("show", Some(json!(args.switch("show"))));
                }
            }
            Tool::App
        }
        "page" if action.starts_with("file-") => {
            if action == "file-write" {
                let Some(revision) = args.flag("revision") else {
                    return Err(error("--revision: the revision `file-read` printed"));
                };
                put("base_revision", Some(json!(revision)));
                put("text", Some(json!(text)));
            } else if let Some(extra) = words.first() {
                return Err(error(&format!("unexpected argument {extra:?}")));
            }
            Tool::Page
        }
        "page" => {
            let Some((page, body)) = words.split_first() else {
                return Err(error("missing <page>"));
            };
            put("page_id", Some(json!(page)));
            if action == "write" {
                let Some(revision) = args.flag("revision") else {
                    return Err(error("--revision: the revision `read` printed"));
                };
                put("base_revision", Some(json!(revision)));
                put("body", Some(json!(body.join(" "))));
                put("title", flag("title"));
            } else if let Some(extra) = body.first() {
                return Err(error(&format!("unexpected argument {extra:?}")));
            }
            Tool::Page
        }
        "ledger" => {
            match action.as_str() {
                "add" => {
                    if text.trim().is_empty() {
                        return Err(error("give a ledger entry, with its kind and source"));
                    }
                    put("text", Some(json!(text)));
                }
                "close" => {
                    let Some((id, how)) = words.split_first() else {
                        return Err(error("close takes <id> \"<how>\""));
                    };
                    let how = how.join(" ");
                    if how.trim().is_empty() {
                        return Err(error("say how it was closed: close <id> \"<how>\""));
                    }
                    put("id", Some(json!(id)));
                    put("how", Some(json!(how)));
                }
                "check" => {
                    let [file] = words.as_slice() else {
                        return Err(error("check takes one <file>"));
                    };
                    // Read here: the file is the caller's, relative to its own folder.
                    let read = std::fs::read_to_string(file)
                        .map_err(|err| error(&format!("couldn't read {file}: {err}")))?;
                    put("file", Some(json!(file)));
                    put("text", Some(json!(read)));
                }
                _ => {
                    if let Some(extra) = words.first() {
                        return Err(error(&format!("unexpected argument {extra:?}")));
                    }
                    if args.switch("open") {
                        put("open", Some(json!(true)));
                    }
                    if args.switch("rules") {
                        put("rules", Some(json!(true)));
                    }
                }
            }
            Tool::Ledger
        }
        "brain" | "transcript" => {
            if action == "read" {
                let [session, from, to] = words.as_slice() else {
                    return Err(error("read takes <session> <from> <to>"));
                };
                let position = |what: &str, word: &str| {
                    word.trim_start_matches('#')
                        .parse::<u32>()
                        .map(|n| json!(n))
                        .map_err(|_| error(&format!("<{what}> is a position, from 0: {word:?}")))
                };
                put("session", Some(json!(session)));
                put("from", Some(position("from", from)?));
                put("to", Some(position("to", to)?));
            } else {
                if text.trim().is_empty() {
                    return Err(error("missing <query>"));
                }
                put("query", Some(json!(text)));
                put("session", flag("session"));
                put("limit", number("limit"));
            }
            match command.name {
                "brain" => Tool::Brain,
                _ => Tool::Transcript,
            }
        }
        "code" => {
            match action.as_str() {
                "search" => {
                    if text.trim().is_empty() {
                        return Err(error("missing <query>"));
                    }
                    put("query", Some(json!(text)));
                    put("kind", flag("kind"));
                    put("language", flag("language"));
                    put("path", flag("path"));
                    put("limit", number("limit"));
                }
                "refs" => {
                    put("symbol", one("symbol", true)?);
                    put("limit", number("limit"));
                }
                _ => {
                    one("nothing", false)?.map_or(Ok(()), |extra| {
                        Err(error(&format!("unexpected argument {extra}")))
                    })?;
                }
            }
            Tool::Code
        }
        "report" => {
            let Some(title) = args.flag("title") else {
                return Err(error("--title: the report's title"));
            };
            put("title", Some(json!(title)));
            put("markdown", Some(json!(args.text("markdown"))));
            Tool::Report
        }
        "feedback" => {
            let (Some(kind), Some(title)) = (args.flag("kind"), args.flag("title")) else {
                return Err(error("--kind (bug or idea) and --title name the report"));
            };
            put("kind", Some(json!(kind)));
            put("title", Some(json!(title)));
            put("body", Some(json!(args.text("body"))));
            put("asked", Some(json!(args.switch("asked"))));
            Tool::Feedback
        }
        _ => {
            let Some(what) = args.flag("action") else {
                return Err(error("--action: what would happen, in one line"));
            };
            put("action", Some(json!(what)));
            put("details", Some(json!(args.text("details"))));
            Tool::Approval
        }
    };
    Ok(CliRequest::Tool {
        tool,
        action,
        args: Value::Object(out),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(words: &[&str]) -> (CliRequest, bool) {
        match parse(words.iter().copied()).unwrap() {
            Invocation::Run {
                command,
                args,
                json,
            } => (request(command, &args).unwrap().unwrap(), json),
            other => panic!("not a run: {other:?}"),
        }
    }

    fn fails(words: &[&str]) -> String {
        match parse(words.iter().copied()) {
            Err(err) => err.message,
            Ok(Invocation::Run { command, args, .. }) => {
                request(command, &args).unwrap_err().message
            }
            Ok(other) => panic!("parsed: {other:?}"),
        }
    }

    #[test]
    fn the_read_commands_parse_into_their_requests() {
        assert_eq!(run(&["sessions"]), (CliRequest::Sessions, false));
        assert_eq!(run(&["workers", "--json"]), (CliRequest::Workers, true));
        assert_eq!(
            run(&["show", "ab12"]).0,
            CliRequest::Show { id: "ab12".into() }
        );
        assert_eq!(
            run(&["read", "--last=5", "ab12", "--json"]),
            (
                CliRequest::Read {
                    id: "ab12".into(),
                    last: Some(5),
                    screen: false
                },
                true
            )
        );
        assert_eq!(
            run(&["read", "ab12", "--screen"]).0,
            CliRequest::Read {
                id: "ab12".into(),
                last: None,
                screen: true
            }
        );
    }

    #[test]
    fn wrong_command_lines_are_usage_errors() {
        assert!(fails(&["nope"]).contains("unknown command"));
        assert!(fails(&["show"]).contains("missing <id>"));
        assert!(fails(&["show", "a", "b"]).contains("unexpected argument"));
        assert!(fails(&["read", "a", "--last"]).contains("needs a N"));
        assert!(fails(&["read", "a", "--last", "0"]).contains("whole number"));
        assert!(fails(&["read", "a", "--last", "2", "--screen"]).contains("don't go together"));
        assert!(fails(&["sessions", "--mine"]).contains("unknown option --mine"));
        assert!(fails(&["start", "--agent", "gpt", "do it"]).contains("one of: claude, codex"));
        assert!(fails(&["preview", "restart"]).contains("not one of"));
        assert!(fails(&["wait", "--timeout", "soon"]).contains("whole number"));
        assert!(fails(&["read", "a", "--screen=yes"]).contains("takes no value"));
    }

    #[test]
    fn the_worker_commands_parse_into_their_requests() {
        assert_eq!(
            run(&[
                "start",
                "--agent",
                "codex",
                "--model",
                "m",
                "--lean",
                "--skill",
                "a",
                "--skill",
                "b",
                "--with",
                "computer-use",
                "write",
                "a",
                "file",
            ])
            .0,
            CliRequest::Start(Box::new(StartRequest {
                title: None,
                agent: Some(Agent::Codex),
                model: Some("m".into()),
                effort: None,
                work: None,
                local: false,
                repo: None,
                base: None,
                permission: None,
                lean: true,
                skills: vec!["a".into(), "b".into()],
                with: vec![Capability::ComputerUse],
                prompt: "write a file".into(),
            }))
        );
        let CliRequest::Start(start) = run(&[
            "start",
            "--local",
            "--permission",
            "full-access",
            "--title",
            "T",
            "go",
        ])
        .0
        else {
            panic!("not a start");
        };
        assert!(start.local);
        assert_eq!(start.repo, None);
        let CliRequest::Start(by_work) = run(&["start", "--work", "Independent review", "go"]).0
        else {
            panic!("not a start");
        };
        assert_eq!(by_work.work.as_deref(), Some("Independent review"));
        assert_eq!(start.permission, Some(Permission::FullAccess));
        assert_eq!(start.title.as_deref(), Some("T"));
        let CliRequest::Start(start) = run(&["start", "--repo", "/src/app", "go"]).0 else {
            panic!("not a start");
        };
        assert_eq!(start.repo.as_deref(), Some("/src/app"));
        let CliRequest::Start(start) = run(&["start", "--repo", "app", "go"]).0 else {
            panic!("not a start");
        };
        let repo = start.repo.unwrap();
        assert!(
            std::path::Path::new(&repo).is_absolute() && repo.ends_with("app"),
            "{repo}"
        );
        assert_eq!(
            run(&["send", "ab12", "keep", "going"]).0,
            CliRequest::Send {
                id: "ab12".into(),
                text: Some("keep going".into()),
                answer: None
            }
        );
        assert_eq!(
            run(&["send", "ab12", "--allow"]).0,
            CliRequest::Send {
                id: "ab12".into(),
                text: None,
                answer: Some(Answer::Allow)
            }
        );
        assert_eq!(
            run(&["wait", "ab12", "cd34", "--timeout", "60"]).0,
            CliRequest::Wait {
                ids: vec!["ab12".into(), "cd34".into()],
                timeout_secs: Some(60)
            }
        );
        assert_eq!(
            run(&["wait"]).0,
            CliRequest::Wait {
                ids: Vec::new(),
                timeout_secs: None
            }
        );
        assert_eq!(
            run(&["close", "ab12"]).0,
            CliRequest::Close { id: "ab12".into() }
        );
        assert!(fails(&["start", "--local", "--worktree", "go"]).contains("don't go together"));
        assert!(fails(&["start", "--local", "--base", "main", "go"]).contains("--base is for"));
        assert!(fails(&["start", "--skill", "a", "go"]).contains("goes with --lean"));
        assert!(fails(&["start", "--with", "computer-use", "go"]).contains("goes with --lean"));
        assert!(
            fails(&["start", "--lean", "--with", "screens", "go"])
                .contains("must be one of: computer-use")
        );
        assert!(fails(&["start", "--lean"]).contains("missing <prompt>"));
        assert!(fails(&["send", "ab12"]).contains("missing <text>"));
        assert!(fails(&["send", "ab12", "--allow", "--deny"]).contains("don't go together"));
        assert!(fails(&["send", "ab12", "--deny", "no"]).contains("take no message"));
    }

    #[test]
    fn models_parses_and_reaches_the_daemon() {
        let Invocation::Run { command, args, .. } = parse(["models", "--json"]).unwrap() else {
            panic!("models did not parse");
        };
        assert!(command.available);
        assert_eq!(request(command, &args).unwrap(), Some(CliRequest::Models));
    }

    fn tool(words: &[&str]) -> (Tool, String, Value) {
        match run(words).0 {
            CliRequest::Tool { tool, action, args } => (tool, action, args),
            other => panic!("not a tool: {other:?}"),
        }
    }

    #[test]
    fn the_kept_tools_parse_into_their_tools_arguments() {
        assert_eq!(
            tool(&[
                "preview",
                "start",
                "--name",
                "web",
                "--env",
                "PORT=3000",
                "--purpose",
                "user",
                "pnpm",
                "dev",
            ]),
            (
                Tool::Preview,
                "start".into(),
                json!({"command": "pnpm dev", "name": "web", "env": {"PORT": "3000"},
                       "purpose": "user"})
            )
        );
        assert_eq!(
            tool(&["preview", "log", "preview-2", "--tail", "9", "--wait", "30"]).2,
            json!({"id": "preview-2", "tail_lines": 9, "wait_secs": 30})
        );
        assert_eq!(tool(&["preview", "stop"]).2, json!({}));
        assert_eq!(
            tool(&["browser", "open", "https://example.com"]).2,
            json!({"url": "https://example.com", "keep": false})
        );
        assert_eq!(
            tool(&["browser", "navigate", "--tab", "t", "--go", "back"]).2,
            json!({"tab": "t", "go": "back"})
        );
        assert_eq!(
            tool(&[
                "browser", "type", "--tab", "t", "e7", "hello", "there", "--submit", "--asks",
                "sign-in"
            ])
            .2,
            json!({"tab": "t", "target": "e7", "text": "hello there", "submit": true,
                   "asks": "sign_in"})
        );
        assert_eq!(
            tool(&["browser", "screenshot", "--tab", "t", "--out", "/x.png"]).2,
            json!({"tab": "t"})
        );
        assert_eq!(
            tool(&["app", "state"]),
            (Tool::App, "state".into(), json!({}))
        );
        assert_eq!(
            tool(&["app", "open", "file", "src/a.rs", "--line", "4", "--show"]).2,
            json!({"target": {"kind": "file", "path": "src/a.rs", "line": 4}, "show": true})
        );
        assert_eq!(
            tool(&["app", "open", "diff", "last-turn"]).2,
            json!({"target": {"kind": "diff", "scope": "last_turn"}, "show": false})
        );
        assert_eq!(
            tool(&[
                "app",
                "terminal-run",
                "--terminal",
                "terminal-2",
                "make",
                "test"
            ])
            .2,
            json!({"command": "make test", "terminal": "terminal-2", "show": false})
        );
        assert_eq!(
            tool(&["page", "write", "p1", "--revision", "r3", "New", "text."]),
            (
                Tool::Page,
                "write".into(),
                json!({"page_id": "p1", "base_revision": "r3", "body": "New text."})
            )
        );
        assert_eq!(
            run(&["ask", "--option", "Yes", "--option", "No", "Ship", "it?"]).0,
            CliRequest::Ask {
                question: "Ship it?".into(),
                options: vec!["Yes".into(), "No".into()],
                title: None
            }
        );
        assert!(fails(&["browser", "read"]).contains("--tab names the tab"));
        assert!(fails(&["browser", "navigate", "--tab", "t"]).contains("missing <url>"));
        assert!(fails(&["preview", "start"]).contains("missing <command>"));
        assert!(fails(&["preview", "stop", "a", "b"]).contains("unexpected argument"));
        assert!(fails(&["app", "open", "diff", "all"]).contains("last-turn, branch"));
        assert!(fails(&["app", "open", "tab"]).contains("is not one of"));
        assert!(fails(&["page", "write", "p1", "text"]).contains("--revision"));
        assert!(fails(&["page", "list"]).contains("not one of"));
        assert!(fails(&["ask", "--option", "Only", "Go?"]).contains("2 to 4 options"));
    }

    /// STRIP-DOWN-PLAN S16c: the thread's, Chat's and side chat's last MCP tools, with the
    /// fields their MCP schemas took.
    #[test]
    fn the_moved_tools_parse_into_their_tools_arguments() {
        assert_eq!(
            tool(&[
                "brain",
                "search",
                "--session",
                "a1b2c3d4",
                "--limit",
                "5",
                "sidebar",
                "modes"
            ]),
            (
                Tool::Brain,
                "search".into(),
                json!({"query": "sidebar modes", "session": "a1b2c3d4", "limit": 5})
            )
        );
        assert_eq!(tool(&["brain", "search", "x"]).2, json!({"query": "x"}));
        assert_eq!(
            tool(&["brain", "read", "a1b2c3d4/2-abcd1234", "#40", "44"]),
            (
                Tool::Brain,
                "read".into(),
                json!({"session": "a1b2c3d4/2-abcd1234", "from": 40, "to": 44})
            )
        );
        assert_eq!(
            tool(&["transcript", "search", "--limit", "3", "the", "budget"]),
            (
                Tool::Transcript,
                "search".into(),
                json!({"query": "the budget", "limit": 3})
            )
        );
        assert_eq!(
            tool(&["transcript", "read", "a1b2c3d4", "0", "3"]),
            (
                Tool::Transcript,
                "read".into(),
                json!({"session": "a1b2c3d4", "from": 0, "to": 3})
            )
        );
        assert_eq!(
            tool(&[
                "code",
                "search",
                "--kind",
                "symbol",
                "--language",
                "rust",
                "--path",
                "crates",
                "--limit",
                "5",
                "ToolHost"
            ]),
            (
                Tool::Code,
                "search".into(),
                json!({"query": "ToolHost", "kind": "symbol", "language": "rust",
                       "path": "crates", "limit": 5})
            )
        );
        assert_eq!(
            tool(&["code", "refs", "cli_call"]).2,
            json!({"symbol": "cli_call"})
        );
        assert_eq!(
            tool(&["code", "map"]),
            (Tool::Code, "map".into(), json!({}))
        );
        assert_eq!(
            tool(&["page", "file-read"]),
            (Tool::Page, "file-read".into(), json!({}))
        );
        assert_eq!(
            tool(&["page", "file-write", "--revision", "r2", "fn", "main()"]).2,
            json!({"base_revision": "r2", "text": "fn main()"})
        );
        assert_eq!(
            tool(&["report", "save", "--title", "Printers", "Bambu", "A1."]),
            (
                Tool::Report,
                "save".into(),
                json!({"title": "Printers", "markdown": "Bambu A1."})
            )
        );
        assert_eq!(
            tool(&[
                "feedback",
                "offer",
                "--kind",
                "bug",
                "--title",
                "Stop spins",
                "--asked",
                "It",
                "kept",
                "spinning."
            ]),
            (
                Tool::Feedback,
                "offer".into(),
                json!({"kind": "bug", "title": "Stop spins", "body": "It kept spinning.",
                       "asked": true})
            )
        );
        assert_eq!(
            tool(&[
                "approval",
                "request",
                "--action",
                "Buy a domain",
                "12",
                "euros",
                "a",
                "year."
            ]),
            (
                Tool::Approval,
                "request".into(),
                json!({"action": "Buy a domain", "details": "12 euros a year."})
            )
        );
        assert!(fails(&["brain", "search"]).contains("missing <query>"));
        assert!(fails(&["brain", "find", "x"]).contains("not one of"));
        assert!(fails(&["brain", "search", "--limit", "0", "x"]).contains("whole number"));
        assert!(fails(&["brain", "read", "s1", "4"]).contains("<session> <from> <to>"));
        assert!(fails(&["transcript", "read", "s1", "x", "4"]).contains("a position"));
        assert!(fails(&["code", "search"]).contains("missing <query>"));
        assert!(fails(&["code", "refs"]).contains("missing <symbol>"));
        assert!(fails(&["code", "map", "x"]).contains("unexpected argument"));
        assert!(fails(&["code", "search", "--kind", "type", "x"]).contains("one of: symbol"));
        assert!(fails(&["page", "file-write", "text"]).contains("--revision"));
        assert!(fails(&["page", "file-read", "x"]).contains("unexpected argument"));
        assert!(fails(&["report", "save", "text"]).contains("--title"));
        assert!(fails(&["feedback", "offer", "--title", "T", "b"]).contains("--kind"));
        assert!(fails(&["feedback", "offer", "--kind", "rant", "b"]).contains("one of: bug"));
        assert!(fails(&["approval", "request", "why"]).contains("--action"));
        assert!(fails(&["approval", "request", "--action", "Pay"]).contains("missing <details>"));
    }

    #[test]
    fn help_and_version_are_asked_for_in_the_usual_ways() {
        assert!(matches!(
            parse(Vec::<String>::new()),
            Ok(Invocation::Help(None))
        ));
        assert!(matches!(parse(["--help"]), Ok(Invocation::Help(None))));
        assert!(matches!(parse(["-h"]), Ok(Invocation::Help(None))));
        assert!(
            matches!(parse(["help", "read"]), Ok(Invocation::Help(Some(c))) if c.name == "read")
        );
        assert!(
            matches!(parse(["read", "--help"]), Ok(Invocation::Help(Some(c))) if c.name == "read")
        );
        assert!(matches!(parse(["--version"]), Ok(Invocation::Version)));
        // After `--`, words are arguments.
        assert!(matches!(
            parse(["send", "ab12", "--", "--help"]),
            Ok(Invocation::Run { .. })
        ));
    }
}
