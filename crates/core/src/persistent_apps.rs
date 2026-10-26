//! Exact literal app invocations. Policy and acceptance corpus are shared with desktop.
use serde::Deserialize;
use std::{collections::HashMap, sync::OnceLock};
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Policy {
    unsafe_assignments: Vec<String>,
    shells: Vec<String>,
    rules: Vec<Rule>,
}
#[derive(Deserialize)]
struct Rule {
    programs: Vec<String>,
    prefixes: Vec<Vec<String>>,
    options: HashMap<String, String>,
    #[serde(default)]
    separator: bool,
    positional: Option<String>,
    #[serde(default)]
    browser: bool,
}
#[derive(PartialEq)]
enum Token {
    Word(String),
    And,
}
fn tokens(command: &str) -> Option<Vec<Token>> {
    let mut chars = command.chars().peekable();
    let mut result = Vec::new();
    let mut word = String::new();
    let mut active = false;
    let mut quoted_name = false;
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if active {
                    if quoted_name && assignment(&word).is_some() {
                        return None;
                    }
                    result.push(Token::Word(std::mem::take(&mut word)));
                    active = false;
                    quoted_name = false;
                }
            }
            '\'' | '"' => {
                active = true;
                if !word.contains('=') {
                    quoted_name = true;
                }
                loop {
                    let next = chars.next()?;
                    if matches!(next, '\r' | '\n' | '\\') || (c == '"' && matches!(next, '$' | '`'))
                    {
                        return None;
                    }
                    if next == c {
                        break;
                    }
                    word.push(next);
                }
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                if active {
                    if quoted_name && assignment(&word).is_some() {
                        return None;
                    }
                    result.push(Token::Word(std::mem::take(&mut word)));
                    active = false;
                    quoted_name = false;
                }
                result.push(Token::And);
            }
            c => {
                if c < ' ' || "\r\n;&|<>()[]{}*?~#$`\\".contains(c) {
                    return None;
                }
                active = true;
                word.push(c);
            }
        }
    }
    if active {
        if quoted_name && assignment(&word).is_some() {
            return None;
        }
        result.push(Token::Word(word));
    }
    Some(result)
}
fn program_name(program: &str) -> Option<&str> {
    if program.contains('/') && !program.starts_with('/') {
        return None;
    }
    program.rsplit('/').next()
}
fn port(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u32>().is_ok_and(|n| n <= 65535)
}
fn assignment(word: &str) -> Option<&str> {
    let (name, _) = word.split_once('=')?;
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }
    Some(name)
}
fn options(rule: &Rule, mut args: &[String]) -> bool {
    let mut browser = false;
    let mut positionals = 0;
    if rule.separator && args.first().is_some_and(|s| s == "--") {
        args = &args[1..];
    }
    let mut at = 0;
    while at < args.len() {
        let arg = &args[at];
        at += 1;
        if !arg.starts_with('-') {
            if rule.positional.as_deref() == Some("port") && positionals == 0 && port(arg) {
                positionals += 1;
                continue;
            }
            if rule.positional.as_deref() == Some("url")
                && (arg == "about:blank"
                    || ((arg.starts_with("http://") || arg.starts_with("https://"))
                        && !arg.chars().any(char::is_whitespace)
                        && arg
                            .split_once("://")
                            .is_some_and(|(_, host)| !host.is_empty())))
            {
                continue;
            }
            return false;
        }
        let (name, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(name, value)| (name, Some(value)));
        let Some(kind) = rule.options.get(name).map(String::as_str) else {
            return false;
        };
        if kind == "flag" {
            if inline.is_some() {
                return false;
            }
            continue;
        }
        if kind == "headless" {
            if inline.is_some_and(|v| v != "new" && v != "old") {
                return false;
            }
            browser = true;
            continue;
        }
        let value = inline.or_else(|| {
            let next = args.get(at).filter(|v| !v.starts_with('-'))?;
            at += 1;
            Some(next.as_str())
        });
        if kind == "optional" && value.is_none() {
            continue;
        }
        let Some(value) = value.filter(|v| !v.is_empty() && !v.starts_with('-')) else {
            return false;
        };
        if kind == "port" && !port(value) {
            return false;
        }
        if kind == "dimensions"
            && !value.split_once(',').is_some_and(|(w, h)| {
                !w.is_empty()
                    && !h.is_empty()
                    && w.bytes().all(|b| b.is_ascii_digit())
                    && h.bytes().all(|b| b.is_ascii_digit())
            })
        {
            return false;
        }
        if name == "--remote-debugging-port" {
            browser = true;
        }
    }
    !rule.browser || browser
}
pub(crate) fn persistent_app(command: &str) -> bool {
    classify(command, 0)
}
fn classify(command: &str, depth: u8) -> bool {
    if depth > 2 {
        return false;
    }
    static POLICY: OnceLock<Policy> = OnceLock::new();
    let policy = POLICY.get_or_init(|| {
        serde_json::from_str(include_str!("persistent_apps.json")).expect("app command policy")
    });
    let Some(tokens) = tokens(command) else {
        return false;
    };
    let mut at = 0;
    let skip_assignments = |at: &mut usize| -> bool {
        while let Some(Token::Word(word)) = tokens.get(*at) {
            let Some(name) = assignment(word) else {
                break;
            };
            if policy.unsafe_assignments.iter().any(|v| v == name) {
                return false;
            }
            *at += 1;
        }
        true
    };
    if !skip_assignments(&mut at) {
        return false;
    }
    while tokens.get(at) == Some(&Token::Word("cd".into())) {
        at += 1;
        if tokens.get(at) == Some(&Token::Word("--".into())) {
            at += 1;
        }
        let Some(Token::Word(directory)) = tokens.get(at) else {
            return false;
        };
        at += 1;
        if directory.is_empty() || directory.starts_with('-') || tokens.get(at) != Some(&Token::And)
        {
            return false;
        }
        at += 1;
        if !skip_assignments(&mut at) {
            return false;
        }
    }
    let Some(words) = tokens[at..]
        .iter()
        .map(|token| match token {
            Token::Word(word) => Some(word.clone()),
            Token::And => None,
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let Some((program, args)) = words.split_first() else {
        return false;
    };
    let Some(name) = program_name(program) else {
        return false;
    };
    if policy.shells.iter().any(|v| v == name) {
        return args.len() == 2
            && matches!(args[0].as_str(), "-c" | "-lc")
            && classify(&args[1], depth + 1);
    }
    policy.rules.iter().any(|rule| {
        rule.programs.iter().any(|v| v == name)
            && rule
                .prefixes
                .iter()
                .any(|prefix| args.starts_with(prefix) && options(rule, &args[prefix.len()..]))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Deserialize)]
    struct Example {
        command: String,
        app: bool,
    }
    #[test]
    fn persistent_app_shared_acceptance_corpus() {
        let examples: Vec<Example> =
            serde_json::from_str(include_str!("../fixtures/persistent_apps.json")).unwrap();
        for example in examples {
            assert_eq!(
                persistent_app(&example.command),
                example.app,
                "{}",
                example.command
            );
        }
    }
    #[test]
    fn persistent_apps_do_not_hold_lead_work_but_unknown_commands_do() {
        use super::super::LiveTurn;
        use brigadier_providers::{BackgroundTask, BackgroundTaskKind, BackgroundTaskStatus};
        for (command, holds) in [
            ("pnpm dev", false),
            ("cd /work && next dev", false),
            ("chromium --headless --remote-debugging-port=9334", false),
            ("pnpm test", true),
            ("pnpm dev && pnpm test", true),
            ("node dev.js", true),
        ] {
            let live = LiveTurn {
                commands: vec![("call".into(), command.into())],
                background: vec![BackgroundTask {
                    id: "job".into(),
                    item_id: Some("call".into()),
                    kind: BackgroundTaskKind::Shell,
                    label: "Dev app".into(),
                    status: BackgroundTaskStatus::Running,
                    started_at_ms: 1,
                    ended_at_ms: None,
                    output_file: None,
                    summary: None,
                    last_line: None,
                }],
                ..Default::default()
            };
            assert_eq!(!live.jobs(2).holding.is_empty(), holds, "{command}");
            assert_eq!(
                live.background.len(),
                1,
                "classification preserves native lifecycle/history"
            );
        }
    }
}
