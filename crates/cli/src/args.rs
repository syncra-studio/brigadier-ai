//! Parses a command line by the table in [`crate::commands`].

use std::collections::BTreeMap;

use crate::commands::{self, Arg, Command};
use crate::protocol::CliRequest;

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

    fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .get(name)
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    fn switch(&self, name: &str) -> bool {
        self.flags.contains_key(name)
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
        other => return Err(error(format!("{other} has no request yet"))),
    }))
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
    fn commands_not_built_yet_parse_but_send_nothing() {
        for words in [
            &[
                "start", "--agent", "codex", "--model", "m", "write", "a", "file",
            ][..],
            &["send", "ab12", "keep", "going"],
            &["wait", "ab12", "cd34", "--timeout", "60"],
            &["wait"],
            &["close", "ab12"],
            &["preview", "start", "pnpm", "dev", "--name", "web"],
            &["browser", "open", "https://example.com"],
            &["ask", "--option", "Yes", "--option", "No", "Ship it?"],
            &["app", "state"],
            &["page", "list"],
            &["models", "--json"],
        ] {
            let Invocation::Run { command, args, .. } = parse(words.iter().copied()).unwrap()
            else {
                panic!("{words:?} did not parse");
            };
            assert!(!command.available, "{words:?}");
            assert_eq!(request(command, &args).unwrap(), None, "{words:?}");
        }
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
