//! Reads what the user pastes or picks to add into [`Item`]s: a `claude mcp add` or
//! `claude mcp add-json` line, a `codex mcp add` line, server JSON (`{"mcpServers": {...}}`, a bare
//! `{"<name>": {...}}` map or one server object), a server URL, a GitHub link, a skill folder, or a
//! skills.sh id, URL or `npx skills add` line.
//!
//! Pure parsing: nothing is fetched or written. A picked folder is only looked into for
//! `SKILL.md`. Values like `${VAR}` stay as written; the Library translates them later.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::Secret;
use crate::item::{Item, ServerSpec, SkillSource, SkillSpec, TransportSpec};

/// Why a paste can't be added, in words for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddError {
    /// Nothing in it is a server or a skill.
    Unrecognized,
    /// A quote is opened and never closed.
    UnclosedQuote,
    /// JSON that doesn't parse.
    Json(String),
    /// A name that isn't letters, digits, `-` and `_`.
    BadName(String),
    /// A single server with no name in the paste or beside it.
    NoName,
    /// JSON with no server in it.
    NoServers,
    /// A line or an entry that is missing a part or has a malformed one.
    Invalid(String),
    /// Something real that Brigadier can't carry over yet.
    Unsupported(String),
    /// A picked path that is not a folder.
    NoFolder(PathBuf),
    /// A folder with no `SKILL.md`, in it or in the folders inside it.
    NoSkill(PathBuf),
}

impl fmt::Display for AddError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unrecognized => f.write_str("That doesn't look like an MCP server or a skill."),
            Self::UnclosedQuote => f.write_str("A quote in that text is never closed."),
            Self::Json(problem) => write!(f, "That JSON doesn't read: {problem}."),
            Self::BadName(name) => write!(
                f,
                "“{name}” can't be a server name: use only letters, digits, - and _."
            ),
            Self::NoName => f.write_str("Give this server a name."),
            Self::NoServers => f.write_str("That JSON has no MCP servers in it."),
            Self::Invalid(message) | Self::Unsupported(message) => f.write_str(message),
            Self::NoFolder(path) => write!(f, "There is no folder at {}.", path.display()),
            Self::NoSkill(path) => write!(
                f,
                "{} has no SKILL.md, in it or in the folders inside it.",
                path.display()
            ),
        }
    }
}

impl std::error::Error for AddError {}

/// The items in a pasted `text`. `name` names a server the paste doesn't (a single server
/// object, a URL); `home` is what `~` stands for.
pub fn parse(text: &str, name: Option<&str>, home: &Path) -> Result<Vec<Item>, AddError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AddError::Unrecognized);
    }
    if text.starts_with('{') {
        return json(text, name);
    }
    // A `"name": {...}` entry copied out of a bigger file; else its error, if nothing else reads.
    let mut entry_error = None;
    if text.starts_with('"') {
        match json(
            &format!("{{{}}}", text.strip_suffix(',').unwrap_or(text)),
            name,
        ) {
            Ok(items) => return Ok(items),
            Err(error) => entry_error = Some(error),
        }
    }
    if let Some(path) = local_path(text, home) {
        return folder(&path, name);
    }
    commands(text, name, home).map_err(|error| match (error, entry_error) {
        (AddError::Unrecognized | AddError::UnclosedQuote, Some(entry_error)) => entry_error,
        (error, _) => error,
    })
}

fn commands(text: &str, name: Option<&str>, home: &Path) -> Result<Vec<Item>, AddError> {
    let mut items = Vec::new();
    for words in &split_commands(text)? {
        items.extend(command(words, name, home)?);
    }
    if items.is_empty() {
        return Err(AddError::Unrecognized);
    }
    Ok(items)
}

/// The skills in a picked folder: a skill folder (with `SKILL.md`), or each skill folder in it.
/// `name` names the skill when there is just one.
pub fn folder(path: &Path, name: Option<&str>) -> Result<Vec<Item>, AddError> {
    let path = if path.file_name().is_some_and(|file| file == "SKILL.md") && path.is_file() {
        path.parent().unwrap_or(path)
    } else {
        path
    };
    if !path.is_dir() {
        return Err(AddError::NoFolder(path.to_owned()));
    }
    let has_skill = |dir: &Path| dir.join("SKILL.md").is_file();
    let dirs = if has_skill(path) {
        vec![path.to_owned()]
    } else {
        let mut children: Vec<PathBuf> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|child| child.is_dir() && has_skill(child))
            .collect();
        children.sort();
        children
    };
    if dirs.is_empty() {
        return Err(AddError::NoSkill(path.to_owned()));
    }
    let name = name.filter(|_| dirs.len() == 1);
    Ok(dirs
        .into_iter()
        .map(|dir| skill(name, SkillSource::Folder(dir)))
        .collect())
}

fn local_path(text: &str, home: &Path) -> Option<PathBuf> {
    if text.contains('\n') {
        return None;
    }
    if text == "~" {
        return Some(home.to_owned());
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    Path::new(text).is_absolute().then(|| PathBuf::from(text))
}

/// One shell command of the paste.
fn command(words: &[String], name: Option<&str>, home: &Path) -> Result<Vec<Item>, AddError> {
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let words = match words.as_slice() {
        ["$" | ">" | "%", rest @ ..] => rest,
        all => all,
    };
    match words {
        [] => Ok(Vec::new()),
        ["claude", "mcp", "add", args @ ..] => claude_add(args).map(|item| vec![item]),
        ["claude", "mcp", "add-json", args @ ..] => claude_add_json(args).map(|item| vec![item]),
        ["codex", "mcp", "add", args @ ..] => codex_add(args).map(|item| vec![item]),
        [one] => single(one, name, home),
        _ => match skills_add_args(words) {
            Some(args) => skills_add(args, home),
            None => Err(AddError::Unrecognized),
        },
    }
}

/// A lone word: a URL, a Git address, a skills.sh id or a path.
fn single(word: &str, name: Option<&str>, home: &Path) -> Result<Vec<Item>, AddError> {
    if let Some(path) = local_path(word, home) {
        return folder(&path, name);
    }
    if word.starts_with("git@") {
        return Ok(vec![skill(
            name,
            SkillSource::Git {
                url: word.to_owned(),
                rev: None,
                subdir: None,
            },
        )]);
    }
    if let Some(url) = Url::parse(word) {
        return url_item(&url, word, name).map(|item| vec![item]);
    }
    let segments: Vec<&str> = word.split('/').collect();
    if segments.first().is_some_and(|first| first.contains('.')) {
        // A link written without its `https://`.
        if let Some(url) = Url::parse(&format!("https://{word}")) {
            return url_item(&url, &format!("https://{word}"), name).map(|item| vec![item]);
        }
    }
    if (2..=3).contains(&segments.len()) && segments.iter().all(|segment| id_part(segment)) {
        return Ok(vec![skill(name, SkillSource::SkillsSh(word.to_owned()))]);
    }
    Err(AddError::Unrecognized)
}

fn skill(name: Option<&str>, source: SkillSource) -> Item {
    Item::Skill(SkillSpec {
        name: name.map(str::to_owned),
        source,
    })
}

fn url_item(url: &Url<'_>, text: &str, name: Option<&str>) -> Result<Item, AddError> {
    let host = url.host.to_ascii_lowercase();
    let segments: Vec<&str> = url
        .path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    if host == "github.com" || host == "www.github.com" {
        return github(&segments, name);
    }
    if host == "skills.sh" || host == "www.skills.sh" {
        if (2..=3).contains(&segments.len()) && segments.iter().all(|segment| id_part(segment)) {
            return Ok(skill(name, SkillSource::SkillsSh(segments.join("/"))));
        }
        return Err(AddError::Invalid(
            "That skills.sh link doesn't point at a skill.".into(),
        ));
    }
    if url.path.ends_with(".git") {
        return Ok(skill(
            name,
            SkillSource::Git {
                url: text.to_owned(),
                rev: None,
                subdir: None,
            },
        ));
    }
    let name = match name {
        Some(name) => checked_name(name)?,
        None => host_name(&host).ok_or(AddError::NoName)?,
    };
    Ok(Item::Server(ServerSpec {
        name,
        transport: TransportSpec::Http {
            url: text.to_owned(),
            headers: Vec::new(),
        },
    }))
}

/// A GitHub repository link, at a revision and in a folder of it when the link has them.
fn github(segments: &[&str], name: Option<&str>) -> Result<Item, AddError> {
    let [owner, repo, rest @ ..] = segments else {
        return Err(AddError::Invalid(
            "That GitHub link doesn't point at a repository.".into(),
        ));
    };
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let url = format!("https://github.com/{owner}/{repo}");
    let (rev, subdir) = match rest {
        [] => (None, Vec::new()),
        ["tree", rev, path @ ..] => (Some(*rev), path.to_vec()),
        ["blob", rev, path @ ..] => {
            // A link to a file: its folder, the skill's when the file is its `SKILL.md`.
            let folder = path.split_last().map_or(&[][..], |(_, folder)| folder);
            (Some(*rev), folder.to_vec())
        }
        _ => {
            return Err(AddError::Invalid(
                "That GitHub link doesn't point at a repository or a folder in one.".into(),
            ));
        }
    };
    let subdir = (!subdir.is_empty()).then(|| subdir.join("/"));
    Ok(skill(
        name,
        SkillSource::Git {
            url,
            rev: rev.map(str::to_owned),
            subdir,
        },
    ))
}

/// A server name from a URL's host: `mcp.linear.app` is `linear`.
fn host_name(host: &str) -> Option<String> {
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return None;
    }
    let mut labels: Vec<&str> = host.split('.').filter(|label| !label.is_empty()).collect();
    if labels.len() > 1 {
        labels.pop();
    }
    if labels.len() > 1
        && matches!(
            labels.last(),
            Some(&("co" | "com" | "org" | "net" | "ac" | "gov"))
        )
    {
        labels.pop();
    }
    let label = labels.last()?;
    let name: String = label
        .chars()
        .map(|c| if name_char(c) { c } else { '-' })
        .collect();
    let name = name.trim_matches('-').to_owned();
    (!name.is_empty()).then_some(name)
}

/// The parts of an `http(s)://` URL the parser needs.
struct Url<'a> {
    host: &'a str,
    path: &'a str,
}

impl<'a> Url<'a> {
    fn parse(text: &'a str) -> Option<Self> {
        let (scheme, rest) = text.split_once("://")?;
        if !scheme.eq_ignore_ascii_case("https") && !scheme.eq_ignore_ascii_case("http") {
            return None;
        }
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..end];
        let path = rest[end..].split(['?', '#']).next().unwrap_or_default();
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        let host = if host.starts_with('[') {
            host.split_once(']').map_or(host, |(ip, _)| ip)
        } else {
            host.split_once(':').map_or(host, |(host, _)| host)
        };
        (!host.is_empty()).then_some(Self { host, path })
    }
}

fn is_url(text: &str) -> bool {
    Url::parse(text).is_some()
}

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

fn id_part(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn checked_name(name: &str) -> Result<String, AddError> {
    if !name.is_empty() && name.chars().all(name_char) {
        Ok(name.to_owned())
    } else {
        Err(AddError::BadName(name.to_owned()))
    }
}

fn env_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `KEY=value`.
fn env_pair(text: &str) -> Option<(String, Secret)> {
    let (key, value) = text.split_once('=')?;
    env_name(key).then(|| (key.to_owned(), Secret::new(value)))
}

/// `Header: value` (not a URL's `scheme://`).
fn header_pair(text: &str) -> Option<(String, Secret)> {
    let (key, value) = text.split_once(':')?;
    let token = !key.is_empty()
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c));
    (token && !value.starts_with("//")).then(|| (key.to_owned(), Secret::new(value.trim())))
}

fn oauth_unsupported() -> AddError {
    AddError::Unsupported(
        "Brigadier can't set an OAuth client for a server yet. Add it without the client \
         options and sign in when the agent asks."
            .into(),
    )
}

fn transport_unsupported(kind: &str) -> AddError {
    AddError::Unsupported(format!(
        "Brigadier can't add a “{kind}” server: only stdio, http and sse ones."
    ))
}

/// A flag of a command line, with its value when written as `--flag=value`.
fn flag(word: &str) -> (&str, Option<&str>) {
    match word.split_once('=') {
        Some((flag, value)) if word.starts_with("--") => (flag, Some(value)),
        _ => (word, None),
    }
}

/// The value of a flag: inline, or the next word.
fn value<'a>(
    flag: &str,
    inline: Option<&'a str>,
    words: &[&'a str],
    at: &mut usize,
) -> Result<&'a str, AddError> {
    if let Some(value) = inline {
        return Ok(value);
    }
    *at += 1;
    words
        .get(*at)
        .copied()
        .ok_or_else(|| AddError::Invalid(format!("The option {flag} needs a value.")))
}

/// `claude mcp add [options] <name> <commandOrUrl> [args...]`.
fn claude_add(words: &[&str]) -> Result<Item, AddError> {
    let mut transport = None;
    let mut env = Vec::new();
    let mut headers = Vec::new();
    let mut positional: Vec<String> = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let word = words[at];
        let (name, inline) = flag(word);
        match name {
            "--" => {
                positional.extend(words[at + 1..].iter().map(|word| (*word).to_owned()));
                break;
            }
            "-t" | "--transport" => {
                transport = Some(value(name, inline, words, &mut at)?.to_ascii_lowercase());
            }
            "-s" | "--scope" => {
                value(name, inline, words, &mut at)?;
            }
            // Both take one or more values: the words after that have their form.
            "-e" | "--env" | "-H" | "--header" => {
                let env_flag = matches!(name, "-e" | "--env");
                let pair = |text: &str| {
                    if env_flag {
                        env_pair(text)
                    } else {
                        header_pair(text)
                    }
                };
                let malformed = |text: &str| {
                    AddError::Invalid(if env_flag {
                        format!("“{text}” isn't an environment variable: write it as KEY=value.")
                    } else {
                        format!("“{text}” isn't a header: write it as \"Name: value\".")
                    })
                };
                let target = if env_flag { &mut env } else { &mut headers };
                let first = value(name, inline, words, &mut at)?;
                target.push(pair(first).ok_or_else(|| malformed(first))?);
                if inline.is_none() {
                    while let Some(next) = words.get(at + 1).and_then(|word| pair(word)) {
                        target.push(next);
                        at += 1;
                    }
                }
            }
            "--client-id" | "--client-secret" | "--callback-port" => {
                return Err(oauth_unsupported());
            }
            _ if word.starts_with('-') && positional.len() < 2 => {
                return Err(AddError::Invalid(format!(
                    "Brigadier doesn't know the option {name} in that line."
                )));
            }
            _ => positional.push(word.to_owned()),
        }
        at += 1;
    }
    let mut positional = positional.into_iter();
    let name = checked_name(&positional.next().ok_or(AddError::NoName)?)?;
    let target = positional.next();
    let rest: Vec<String> = positional.collect();
    let http = match transport.as_deref() {
        Some("http" | "sse") => true,
        Some("stdio") => false,
        Some(other) => return Err(transport_unsupported(other)),
        // Defaults to stdio; a lone URL can only mean a remote server.
        None => rest.is_empty() && target.as_deref().is_some_and(is_url),
    };
    let transport = if http {
        let url = target.ok_or_else(|| missing_url(&name))?;
        url_only(&name, &url, &rest, &env)?;
        TransportSpec::Http { url, headers }
    } else {
        if !headers.is_empty() {
            return Err(headers_need_url());
        }
        TransportSpec::Stdio {
            command: target.ok_or_else(|| missing_command(&name))?,
            args: rest,
            env,
            cwd: None,
        }
    };
    Ok(Item::Server(ServerSpec { name, transport }))
}

fn url_only(
    name: &str,
    url: &str,
    rest: &[String],
    env: &[(String, Secret)],
) -> Result<(), AddError> {
    if !is_url(url) {
        return Err(AddError::Invalid(format!(
            "The address of “{name}” must start with https:// or http://."
        )));
    }
    if !rest.is_empty() {
        return Err(AddError::Invalid(format!(
            "“{name}” is reached by URL: there is nothing to put after the URL."
        )));
    }
    if !env.is_empty() {
        return Err(AddError::Invalid(
            "Environment variables only go with a server the agent starts, not one reached by URL."
                .into(),
        ));
    }
    Ok(())
}

fn headers_need_url() -> AddError {
    AddError::Invalid("Headers only go with a server reached by URL.".into())
}

fn missing_url(name: &str) -> AddError {
    AddError::Invalid(format!("“{name}” has no URL."))
}

fn missing_command(name: &str) -> AddError {
    AddError::Invalid(format!("“{name}” has no command to start it."))
}

/// `claude mcp add-json [options] <name> <json>`.
fn claude_add_json(words: &[&str]) -> Result<Item, AddError> {
    let mut positional = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let (name, inline) = flag(words[at]);
        match name {
            "-s" | "--scope" => {
                value(name, inline, words, &mut at)?;
            }
            "--client-secret" => return Err(oauth_unsupported()),
            _ => positional.push(words[at]),
        }
        at += 1;
    }
    let [name, json] = positional[..] else {
        return Err(AddError::Invalid(
            "claude mcp add-json takes a name and then the server's JSON.".into(),
        ));
    };
    let name = checked_name(name)?;
    let value: Value =
        serde_json::from_str(json).map_err(|error| AddError::Json(error.to_string()))?;
    let Value::Object(object) = value else {
        return Err(AddError::Json("the server must be an object".into()));
    };
    json_server(name, &object)
}

/// `codex mcp add [options] <name> (--url <url> | -- <command>...)`.
fn codex_add(words: &[&str]) -> Result<Item, AddError> {
    let mut env = Vec::new();
    let mut url = None;
    let mut bearer = None;
    let mut positional: Vec<String> = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let word = words[at];
        let (name, inline) = flag(word);
        match name {
            "--" => {
                positional.extend(words[at + 1..].iter().map(|word| (*word).to_owned()));
                break;
            }
            "--env" => {
                let text = value(name, inline, words, &mut at)?;
                env.push(env_pair(text).ok_or_else(|| {
                    AddError::Invalid(format!(
                        "“{text}” isn't an environment variable: write it as KEY=value."
                    ))
                })?);
            }
            "--url" => url = Some(value(name, inline, words, &mut at)?.to_owned()),
            "--bearer-token-env-var" => {
                let var = value(name, inline, words, &mut at)?;
                if !env_name(var) {
                    return Err(AddError::Invalid(format!(
                        "“{var}” isn't an environment variable name."
                    )));
                }
                bearer = Some(var.to_owned());
            }
            // Settings of the Codex run itself, not of the server.
            "-c" | "--config" | "--enable" | "--disable" => {
                value(name, inline, words, &mut at)?;
            }
            "--oauth-client-id"
            | "--oauth-client-secret"
            | "--oauth-client-registration"
            | "--oauth-resource" => return Err(oauth_unsupported()),
            _ if word.starts_with('-') && positional.len() < 2 => {
                return Err(AddError::Invalid(format!(
                    "Brigadier doesn't know the option {name} in that line."
                )));
            }
            _ => positional.push(word.to_owned()),
        }
        at += 1;
    }
    let mut positional = positional.into_iter();
    let name = checked_name(&positional.next().ok_or(AddError::NoName)?)?;
    let command: Vec<String> = positional.collect();
    let transport = match url {
        Some(url) => {
            if !command.is_empty() {
                return Err(AddError::Invalid(format!(
                    "“{name}” has both a URL and a command: give one."
                )));
            }
            url_only(&name, &url, &[], &env)?;
            let headers = bearer
                .map(|var| {
                    (
                        "Authorization".to_owned(),
                        Secret::new(format!("Bearer ${{{var}}}")),
                    )
                })
                .into_iter()
                .collect();
            TransportSpec::Http { url, headers }
        }
        None => {
            if bearer.is_some() {
                return Err(AddError::Invalid(
                    "A bearer token only goes with a server reached by URL.".into(),
                ));
            }
            let mut command = command.into_iter();
            TransportSpec::Stdio {
                command: command.next().ok_or_else(|| missing_command(&name))?,
                args: command.collect(),
                env,
                cwd: None,
            }
        }
    };
    Ok(Item::Server(ServerSpec { name, transport }))
}

/// The words after `skills add` when the line runs the skills CLI (`npx skills add`, ...).
fn skills_add_args<'a>(words: &'a [&'a str]) -> Option<&'a [&'a str]> {
    let mut rest = match words {
        ["npx" | "bunx" | "pnpx", rest @ ..] => rest,
        ["pnpm" | "yarn" | "bun", "dlx" | "x", rest @ ..] => rest,
        ["npm", "exec", rest @ ..] => rest,
        rest => rest,
    };
    while let [first, tail @ ..] = rest {
        if !matches!(*first, "-y" | "--yes") {
            break;
        }
        rest = tail;
    }
    match rest {
        [cli, "add", args @ ..] if *cli == "skills" || cli.starts_with("skills@") => Some(args),
        _ => None,
    }
}

/// `skills add <source> [--skill <name>]...`.
fn skills_add(words: &[&str], home: &Path) -> Result<Vec<Item>, AddError> {
    let mut source = None;
    let mut picked = Vec::new();
    let mut at = 0;
    while at < words.len() {
        let (name, inline) = flag(words[at]);
        match name {
            "-s" | "--skill" => picked.push(value(name, inline, words, &mut at)?),
            "-a" | "--agent" => {
                value(name, inline, words, &mut at)?;
            }
            _ if name.starts_with('-') => {}
            _ if source.is_none() => source = Some(words[at]),
            _ => return Err(AddError::Unrecognized),
        }
        at += 1;
    }
    let source = source.ok_or(AddError::Unrecognized)?;
    let segments: Vec<&str> = source.split('/').collect();
    let id = segments.len() == 2 && segments.iter().all(|segment| id_part(segment));
    if picked.is_empty() {
        return single(source, None, home);
    }
    picked
        .into_iter()
        .map(|skill_name| {
            if id {
                Ok(skill(
                    None,
                    SkillSource::SkillsSh(format!("{source}/{skill_name}")),
                ))
            } else {
                let mut items = single(source, Some(skill_name), home)?;
                match items.pop() {
                    Some(Item::Skill(spec)) if items.is_empty() => Ok(Item::Skill(spec)),
                    _ => Err(AddError::Unrecognized),
                }
            }
        })
        .collect()
}

/// Server JSON: `{"mcpServers": {...}}`, `{"<name>": {...}}` or one server object.
fn json(text: &str, name: Option<&str>) -> Result<Vec<Item>, AddError> {
    let value: Value =
        serde_json::from_str(text).map_err(|error| AddError::Json(error.to_string()))?;
    let Value::Object(object) = value else {
        return Err(AddError::NoServers);
    };
    let servers = ["mcpServers", "servers"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_object))
        .filter(|servers| !is_server(servers));
    if let Some(servers) = servers {
        return json_map(servers);
    }
    if is_server(&object) {
        let name = checked_name(name.ok_or(AddError::NoName)?)?;
        return Ok(vec![json_server(name, &object)?]);
    }
    if object.is_empty() || !object.values().all(Value::is_object) {
        return Err(AddError::NoServers);
    }
    json_map(&object)
}

fn json_map(servers: &Map<String, Value>) -> Result<Vec<Item>, AddError> {
    if servers.is_empty() {
        return Err(AddError::NoServers);
    }
    servers
        .iter()
        .map(|(name, server)| {
            let server = server
                .as_object()
                .ok_or_else(|| AddError::Json(format!("the server “{name}” must be an object")))?;
            json_server(checked_name(name)?, server)
        })
        .collect()
}

const URL_KEYS: [&str; 3] = ["url", "serverUrl", "httpUrl"];

fn is_server(object: &Map<String, Value>) -> bool {
    ["command", "type"]
        .iter()
        .chain(&URL_KEYS)
        .any(|key| object.get(*key).is_some_and(|value| !value.is_object()))
}

fn json_server(name: String, object: &Map<String, Value>) -> Result<Item, AddError> {
    let oauth_client = object
        .get("oauth")
        .and_then(Value::as_object)
        .is_some_and(|oauth| oauth.contains_key("clientId") || oauth.contains_key("client_id"));
    if oauth_client {
        return Err(oauth_unsupported());
    }
    let text = |key: &str| -> Result<Option<String>, AddError> {
        match object.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => scalar(value)
                .map(Some)
                .ok_or_else(|| AddError::Json(format!("“{key}” of “{name}” must be text"))),
        }
    };
    let kind = text("type")?.map(|kind| kind.to_ascii_lowercase());
    let url = URL_KEYS
        .iter()
        .find_map(|key| text(key).transpose())
        .transpose()?;
    let command = text("command")?;
    let http = match kind.as_deref() {
        Some("http" | "sse" | "streamable-http" | "streamablehttp") => true,
        Some("stdio") => false,
        Some(other) => return Err(transport_unsupported(other)),
        None if command.is_some() => false,
        None if url.is_some() => true,
        None => {
            return Err(AddError::Invalid(format!(
                "“{name}” has neither a command nor a URL."
            )));
        }
    };
    let transport = if http {
        let url = url.ok_or_else(|| missing_url(&name))?;
        url_only(&name, &url, &[], &[])?;
        TransportSpec::Http {
            url,
            headers: pairs(object, "headers", &name)?,
        }
    } else {
        let args = match object.get("args") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(args)) => args
                .iter()
                .map(|arg| {
                    scalar(arg).ok_or_else(|| {
                        AddError::Json(format!("the arguments of “{name}” must be text"))
                    })
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(AddError::Json(format!(
                    "the arguments of “{name}” must be a list"
                )));
            }
        };
        if pairs(object, "headers", &name).is_ok_and(|headers| !headers.is_empty()) {
            return Err(headers_need_url());
        }
        TransportSpec::Stdio {
            command: command.ok_or_else(|| missing_command(&name))?,
            args,
            env: pairs(object, "env", &name)?,
            cwd: text("cwd")?.map(PathBuf::from),
        }
    };
    Ok(Item::Server(ServerSpec { name, transport }))
}

/// A JSON string, number or boolean as text.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn pairs(
    object: &Map<String, Value>,
    key: &str,
    name: &str,
) -> Result<Vec<(String, Secret)>, AddError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                let value = scalar(value).ok_or_else(|| {
                    AddError::Json(format!("the value of “{key}” in “{name}” must be text"))
                })?;
                Ok((key.clone(), Secret::new(value)))
            })
            .collect(),
        Some(_) => Err(AddError::Json(format!(
            "“{key}” of “{name}” must be an object"
        ))),
    }
}

/// The shell commands in a paste, as words: quotes, backslash escapes and line continuations
/// read as a shell does; a newline, `;` or `&&` ends a command and `#` starts a comment.
fn split_commands(text: &str) -> Result<Vec<Vec<String>>, AddError> {
    let mut commands = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    let end_word = |words: &mut Vec<String>, word: &mut String, in_word: &mut bool| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err(AddError::UnclosedQuote),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some('\n') => {}
                            Some(c @ ('"' | '\\' | '$' | '`')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err(AddError::UnclosedQuote),
                        },
                        Some(c) => word.push(c),
                        None => return Err(AddError::UnclosedQuote),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {}
                Some('\r') if chars.peek() == Some(&'\n') => {
                    chars.next();
                }
                Some(c) => {
                    in_word = true;
                    word.push(c);
                }
                None => {}
            },
            '#' if !in_word => while chars.next_if(|c| *c != '\n').is_some() {},
            '\n' | ';' => {
                end_word(&mut words, &mut word, &mut in_word);
                commands.push(std::mem::take(&mut words));
            }
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                end_word(&mut words, &mut word, &mut in_word);
                commands.push(std::mem::take(&mut words));
            }
            c if c.is_whitespace() => end_word(&mut words, &mut word, &mut in_word),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    end_word(&mut words, &mut word, &mut in_word);
    commands.push(words);
    commands.retain(|words| !words.is_empty());
    Ok(commands)
}

#[cfg(test)]
mod tests;
