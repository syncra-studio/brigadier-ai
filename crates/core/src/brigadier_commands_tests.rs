//! The approval policy's list of Brigadier's own commands, which a level runs unasked
//! (`policy::BRIGADIER_COMMANDS`), against the CLI they name: every command the daemon answers
//! is on it, and none that `brigadier` runs itself before any grant is checked
//! (`secrets::LOCAL_COMMANDS`), today `mcp run`, which starts a program, and `secret` (#737).

use brigadier_cli::args::{self, Invocation};
use brigadier_cli::commands::{self, Arg, Command};
use brigadier_cli::secrets;
use brigadier_providers::policy;

/// Words that may follow a command's first words.
const TAILS: &[&[&str]] = &[
    &[],
    &["x"],
    &["--json"],
    &["run", "a/b", "--", "calc.exe"],
    &["a/b", "--", "calc.exe"],
    &["headers", "a/b"],
    &["--", "run", "a/b"],
];

/// The actions a command takes as its first word (`mcp list`), if it has a fixed set.
fn actions(command: &Command) -> &'static [&'static str] {
    command
        .args
        .iter()
        .find_map(|arg| match arg {
            Arg::Positional { choices, .. } => Some(*choices),
            Arg::Flag { .. } => None,
        })
        .unwrap_or(&[])
}

fn words<'a>(first: &'a str, tail: &[&'a str]) -> Vec<&'a str> {
    first.split(' ').chain(tail.iter().copied()).collect()
}

#[test]
fn the_policy_lists_every_command_the_daemon_answers() {
    for entry in policy::BRIGADIER_COMMANDS {
        let (name, action) = entry
            .split_once(' ')
            .map_or((*entry, None), |(name, action)| (name, Some(action)));
        let command =
            commands::find(name).unwrap_or_else(|| panic!("`{entry}` is no command of the CLI"));
        if let Some(action) = action {
            assert!(actions(command).contains(&action), "`{entry}`");
        }
    }
    for command in commands::COMMANDS {
        let listed: Vec<&str> = (policy::BRIGADIER_COMMANDS.iter().copied())
            .filter(|entry| entry.split(' ').next() == Some(command.name))
            .collect();
        assert!(
            !listed.is_empty(),
            "`{}` is not on policy::BRIGADIER_COMMANDS: a session's level would ask for it",
            command.name
        );
        // A command listed by its actions: every action the CLI takes.
        if listed.iter().any(|entry| entry.contains(' ')) {
            for action in actions(command) {
                let entry = format!("{} {action}", command.name);
                assert!(listed.iter().any(|listed| *listed == entry), "`{entry}`");
            }
        }
    }
}

/// #737: a command `brigadier` runs itself (a future `context run <program>` too) is never one
/// a level runs unasked, by the policy or by a CLI's own allow rules.
#[test]
fn no_command_brigadier_runs_itself_is_approved() {
    for local in secrets::LOCAL_COMMANDS {
        let local = local.join(" ");
        for tail in TAILS {
            let words = words(&local, tail);
            assert!(secrets::is_local(&words), "{words:?}");
            assert!(!policy::brigadier_answers(&words), "{words:?}");
        }
    }
    let (prefixes, exact) = policy::brigadier_rules();
    let lines = (policy::BRIGADIER_COMMANDS.iter().copied())
        .chain(policy::BRIGADIER_HELP.iter().copied())
        .map(str::to_owned)
        .chain(
            prefixes
                .iter()
                .chain(&exact)
                .map(|line| line.trim_start_matches("brigadier").trim().to_owned()),
        )
        .collect::<Vec<_>>();
    for line in &lines {
        for tail in TAILS {
            let words: Vec<&str> = words(line, tail)
                .into_iter()
                .filter(|word| !word.is_empty())
                .collect();
            assert!(
                !policy::brigadier_answers(&words) || !secrets::is_local(&words),
                "{words:?} is approved, but runs locally"
            );
        }
    }
    // A CLI's prefix rule matches the start of a command line, not always at a word's end; an
    // exact one the whole line.
    for local in secrets::LOCAL_COMMANDS {
        let local = format!("brigadier {}", local.join(" "));
        for rule in &prefixes {
            assert!(
                !local.starts_with(rule.as_str()),
                "`{rule}` matches `{local}`"
            );
        }
        for tail in TAILS {
            let line = [local.as_str()]
                .iter()
                .chain(*tail)
                .copied()
                .collect::<Vec<_>>();
            assert!(!exact.contains(&line.join(" ")), "{line:?}");
        }
    }
}

/// The help and version words print and run nothing, as `brigadier` reads them: they come
/// before any command, and a command's `--help` and `-h` too.
#[test]
fn the_help_words_brigadier_runs_unasked_only_print() {
    let prints = |words: &[&str]| {
        matches!(
            args::parse(words.iter().copied()),
            Ok(Invocation::Help(_) | Invocation::Topic(_) | Invocation::Version)
        )
    };
    assert!(prints(&[]));
    assert!(policy::brigadier_answers::<&str>(&[]));
    for word in policy::BRIGADIER_HELP {
        assert!(prints(&[*word]), "{word}");
        assert!(policy::brigadier_answers(&[*word]), "{word}");
    }
    assert!(prints(&["help", "wait"]));
    for help in ["--help", "-h"] {
        assert!(prints(&["mcp", help]), "{help}");
        assert!(policy::brigadier_answers(&["mcp", help]), "{help}");
    }
}
