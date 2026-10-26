//! Action evidence only. This is a deliberately bounded shell grammar, never a shell runner.
//! Historical HEAD/branch comes from recorded output, not from today's checkout. Local git
//! is consulted only for repository config whose files predate the observed action.
use super::super::gh::Gh;
use crate::PrIdentity;
use brigadier_providers::{ProviderEvent, model::ItemStatus};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Repo {
    pub host: String,
    pub owner: String,
    pub name: String,
}
impl Repo {
    fn parse(value: &str, host: &str) -> Option<Self> {
        let value = value
            .trim()
            .trim_end_matches('/')
            .strip_suffix(".git")
            .unwrap_or(value.trim().trim_end_matches('/'));
        let value = if let Some(rest) = value.strip_prefix("git@") {
            rest.replacen(':', "/", 1)
        } else if let Some(rest) = value.strip_prefix("ssh://git@") {
            rest.to_owned()
        } else {
            value.strip_prefix("https://").unwrap_or(value).to_owned()
        };
        let parts = value.split('/').collect::<Vec<_>>();
        let url = match parts.as_slice() {
            [owner, name] => format!("https://{host}/{owner}/{name}/pull/1"),
            [host, owner, name] => format!("https://{host}/{owner}/{name}/pull/1"),
            _ => return None,
        };
        let pr = PrIdentity::parse(&url)?;
        Some(Self {
            host: pr.host,
            owner: pr.owner,
            name: pr.repo,
        })
    }
    fn from_pr(pr: &PrIdentity) -> Self {
        Self {
            host: pr.host.clone(),
            owner: pr.owner.clone(),
            name: pr.repo.clone(),
        }
    }
    fn pr(&self, number: u32) -> PrIdentity {
        PrIdentity {
            host: self.host.clone(),
            owner: self.owner.clone(),
            repo: self.name.clone(),
            number,
        }
    }
}
#[derive(Clone, Debug)]
pub(super) enum Lookup {
    Number(Repo, u32),
    Branch {
        repos: Vec<Repo>,
        branch: String,
        head_repo: Repo,
        oid: String,
        at: i64,
    },
    Run(Repo, u64, i64),
}
#[derive(Clone, Debug)]
pub(super) struct Action {
    pub identity: Option<PrIdentity>,
    pub lookup: Option<Lookup>,
    pub account: Option<String>,
    pub action: String,
    pub item: String,
    pub at: Option<i64>,
}
#[derive(Clone, Default)]
struct Evidence {
    repo: Option<Repo>,
    branch: Option<String>,
    oid: Option<String>,
    branch_at: Option<i64>,
    oid_at: Option<i64>,
}
#[derive(Default)]
pub(super) struct Reader {
    evidence: HashMap<PathBuf, Evidence>,
    wrapper_paths: HashMap<String, PathBuf>,
}
struct Invocation {
    words: Vec<String>,
    cwd: Option<PathBuf>,
    account: Option<String>,
    host: String,
    repo: Option<String>,
}
fn login(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && value.as_bytes()[0].is_ascii_alphanumeric()
}
fn branch(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.contains("..")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
}
fn oid(value: &str) -> bool {
    (7..=64).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn path(base: Option<&Path>, value: &str) -> Option<PathBuf> {
    let p = PathBuf::from(value);
    if p.is_absolute() {
        Some(p)
    } else {
        Some(base?.join(p))
    }
}
fn unchanged_before(file: &Path, at: Option<i64>) -> bool {
    let Some(at) = at else { return false };
    std::fs::metadata(file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .is_some_and(|d| d.as_millis() <= at as u128)
}
/// Quotes protect data; operators/substitutions remain operators. No eval, pipelines,
/// suffix commands, dynamic refs, process substitutions, or unquoted glob expansion.
fn words(text: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for c in text.chars() {
        if escaped {
            word.push(c);
            escaped = false;
            started = true;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if q == '"' && matches!(c, '$' | '`') {
                return None;
            } else {
                word.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                started = true;
            }
            ';' | '&' | '|' | '>' | '<' | '$' | '`' | '\n' | '(' | ')' | '{' | '}' | '*' | '?'
            | '[' | ']' | '~' => return None,
            c if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            _ => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() || escaped {
        return None;
    }
    if started {
        out.push(word);
    }
    Some(out)
}
/// Only named `gh auth token` substitutions are decoded, as account metadata. Literal token
/// values and other substitutions cause rejection and never enter an Action or diagnostic.
fn credentials(text: &str) -> Option<(String, Option<String>, Option<String>)> {
    static AUTH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?:GH_TOKEN|GITHUB_TOKEN|GH_ENTERPRISE_TOKEN|GITHUB_ENTERPRISE_TOKEN)=(?:"\$\((?P<quoted>gh auth token [^()\n]+)\)"|\$\((?P<plain>gh auth token [^()\n]+)\))"#).unwrap()
    });
    let mut account = None;
    let mut host = None;
    let mut result = text.to_owned();
    for capture in AUTH.captures_iter(text) {
        if account.is_some() {
            return None;
        }
        let args = words(
            capture
                .name("quoted")
                .or_else(|| capture.name("plain"))?
                .as_str(),
        )?;
        let mut i = 3;
        while i < args.len() {
            let (key, value) = if let Some((k, v)) = args[i].split_once('=') {
                (k, v)
            } else {
                let key = args[i].as_str();
                i += 1;
                (key, args.get(i)?.as_str())
            };
            match key {
                "--user" if login(value) => account = Some(value.to_owned()),
                "--hostname" if !value.contains('@') => host = Some(value.to_owned()),
                _ => return None,
            };
            i += 1;
        }
        account.as_ref()?;
        result = result.replace(capture.get(0)?.as_str(), "BRIGADIER_NAMED_GH=1");
    }
    Some((result, account, host))
}
fn invocation(text: &str, cwd: Option<&Path>) -> Option<Invocation> {
    // The only accepted compound prefix is one literal `cd … &&`: an overall zero exit
    // proves both the directory change and the one final action succeeded.
    static CD: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^\s*cd\s+(?P<dir>.+?)\s*&&\s*(?P<rest>.+)$").unwrap()
    });
    let mut cwd = cwd.map(Path::to_path_buf);
    let mut text = text;
    if let Some(capture) = CD.captures(text) {
        let dirs = words(capture.name("dir")?.as_str())?;
        if dirs.len() != 1 {
            return None;
        }
        cwd = path(cwd.as_deref(), &dirs[0]);
        text = capture.name("rest")?.as_str();
    }
    let (safe, account, named_host) = credentials(text)?;
    let mut argv = words(&safe)?;
    if argv.first().is_some_and(|w| w == "env") {
        argv.remove(0);
    }
    let mut host = named_host.unwrap_or_else(|| "github.com".into());
    let mut repo = None;
    while argv.first().is_some_and(|w| w.contains('=')) {
        let (key, value) = argv
            .remove(0)
            .split_once('=')
            .map(|(k, v)| (k.to_owned(), v.to_owned()))?;
        match key.as_str() {
            "BRIGADIER_NAMED_GH" => {}
            "GH_HOST" => host = value,
            "GH_REPO" => repo = Some(value),
            "GH_CONFIG_DIR" => {}
            _ => return None,
        }
    }
    Some(Invocation {
        words: argv,
        cwd,
        account,
        host,
        repo,
    })
}
fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    // Fixed read-only commands, never text reconstructed from the transcript.
    let mut command = std::process::Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(cwd)
        .args(args);
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|k| k.starts_with("GIT_")) {
            command.env_remove(key);
        }
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
fn remote(cwd: Option<&Path>, name: &str, at: Option<i64>) -> Option<Repo> {
    if !branch(name) {
        return None;
    }
    let cwd = cwd?;
    let common = git(cwd, &["rev-parse", "--git-common-dir"])?;
    let common = path(Some(cwd), &common)?;
    if !unchanged_before(&common.join("config"), at) {
        return None;
    }
    let local =
        git(cwd, &["rev-parse", "--git-path", "config.worktree"]).and_then(|p| path(Some(cwd), &p));
    if local
        .as_ref()
        .is_some_and(|p| p.exists() && !unchanged_before(p, at))
    {
        return None;
    }
    let mut configs = Vec::new();
    if let Some(local) = local.filter(|p| p.exists()) {
        configs.push(local);
    }
    configs.push(common.join("config"));
    for config in configs {
        let text = std::fs::read_to_string(&config).ok()?;
        if text
            .lines()
            .any(|line| line.trim().to_ascii_lowercase().starts_with("[include"))
        {
            return None;
        }
        let config = config.to_str()?;
        let key = format!("remote.{name}.url");
        if let Some(value) = git(
            cwd,
            &["config", "--file", config, "--no-includes", "--get", &key],
        ) {
            return Repo::parse(&value, "github.com");
        }
    }
    None
}
fn canonical(value: &str) -> Option<PrIdentity> {
    PrIdentity::parse(value.split(['#', '?']).next()?.trim_end_matches('/'))
}
fn response(output: &str, repo: Option<&Repo>, host: &str) -> Option<PrIdentity> {
    let mut found = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        for key in ["html_url", "pull_request_url"] {
            if let Some(url) = value[key].as_str()
                && let Some(pr) =
                    canonical(url).or_else(|| api_identity(url, host).map(|(r, n)| r.pr(n)))
            {
                found.push(pr);
            }
        }
        if let Some(url) = value
            .pointer("/pull_request/html_url")
            .and_then(Value::as_str)
            .and_then(canonical)
        {
            found.push(url);
        }
    }
    for line in output.lines().map(str::trim) {
        if let Some(pr) = canonical(line) {
            found.push(pr);
            continue;
        }
        // gh's mutation receipts, not arbitrary text or a title/body URL.
        if (line.starts_with("✓ ") || line.starts_with("✔ "))
            && let Some((_, tail)) = line.split_once("pull request ")
            && let Some(id) = tail.split_whitespace().next()
            && let Some((r, n)) = id.split_once('#')
            && let (Some(r), Ok(n)) = (Repo::parse(r, host), n.parse::<u32>())
        {
            found.push(r.pr(n));
        }
    }
    found.retain(|pr| repo.is_none_or(|r| Repo::from_pr(pr) == *r));
    let keys = found.iter().map(PrIdentity::key).collect::<HashSet<_>>();
    (keys.len() == 1).then(|| found.remove(0))
}
fn api_identity(value: &str, host: &str) -> Option<(Repo, u32)> {
    let endpoint = value
        .strip_prefix("https://api.github.com/")
        .map(|v| ("github.com", v))
        .or_else(|| {
            value
                .strip_prefix("https://")
                .and_then(|v| v.split_once("/api/v3/"))
        })
        .unwrap_or((host, value.trim_start_matches('/')));
    let parts = endpoint.1.split('/').collect::<Vec<_>>();
    if parts.len() < 5 || parts[0] != "repos" || !matches!(parts[3], "pulls" | "issues") {
        return None;
    }
    let r = Repo::parse(&format!("{}/{}/{}", endpoint.0, parts[1], parts[2]), host)?;
    Some((r, parts[4].parse::<u32>().ok().filter(|n| *n > 0)?))
}

fn wrapper(text: &str, cwd: Option<&Path>) -> Option<Invocation> {
    let lines = text
        .lines()
        .filter(|line| {
            !line.trim().is_empty()
                && !line.trim().starts_with('#')
                && !matches!(line.trim(), "set -e" | "set -eu" | "set -euo pipefail")
        })
        .collect::<Vec<_>>();
    let body = lines.join(" ");
    let body = body.trim().strip_prefix("exec ").unwrap_or(body.trim());
    let body = body.strip_suffix("\"$@\"")?;
    // Single-quoted '$@' would forward a literal argument, never the caller's argv.
    if text.trim_end().ends_with("'$@'") {
        return None;
    }
    let inv = invocation(body.trim(), cwd)?;
    (inv.words.first().is_some_and(|p| p == "gh") && inv.account.is_some()).then_some(inv)
}
fn watch_loop(text: &str, cwd: Option<&Path>) -> Option<Invocation> {
    static LOOP: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"^\s*(?:until|while\s+!)\s+(?P<read>.+?);\s*do\s+sleep\s+[0-9]+[smh]?\s*;\s*done\s*$",
        )
        .unwrap()
    });
    let capture = LOOP.captures(text)?;
    let mut inv = invocation(capture.name("read")?.as_str(), cwd)?;
    if inv.words.first()?.as_str() != "gh"
        || inv.words.get(1)?.as_str() != "pr"
        || !matches!(inv.words.get(2)?.as_str(), "view" | "checks")
    {
        return None;
    }
    inv.words[2] = "checks".into();
    inv.words.push("--watch".into());
    Some(inv)
}
fn inline_wrapper(text: &str, cwd: Option<&Path>) -> Option<Invocation> {
    static FUNCTION: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"(?s)^\s*(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\(\)\s*\{(?P<body>[^{}]+)\}\s*;\s*(?P<call>.+)$").unwrap()
    });
    let c = FUNCTION.captures(text)?;
    let call = words(c.name("call")?.as_str())?;
    if call.first()?.as_str() != c.name("name")?.as_str() {
        return None;
    }
    let body = c.name("body")?.as_str().trim().trim_end_matches(';').trim();
    let mut inv = wrapper(body, cwd)?;
    inv.words.extend(call.into_iter().skip(1));
    Some(inv)
}
impl Reader {
    pub(super) fn observe(
        &mut self,
        event: &ProviderEvent,
        fallback_cwd: Option<&Path>,
        at: Option<i64>,
        started_at: Option<i64>,
        captured_head: Option<&str>,
    ) -> Option<Action> {
        let ProviderEvent::Command {
            item_id,
            command,
            cwd,
            status,
            exit_code,
            output,
            ..
        } = event
        else {
            return None;
        };
        let base = cwd.as_deref().map(Path::new).or(fallback_cwd);
        if *status != ItemStatus::Completed || *exit_code != Some(0) {
            self.evidence.clear();
            return None;
        }
        let mut inv = if let Some(inv) = inline_wrapper(command, base) {
            inv
        } else if let Some(inv) = watch_loop(command, base) {
            inv
        } else if let Some(inv) = invocation(command, base) {
            inv
        } else {
            self.evidence.clear();
            return None;
        };
        if inv.words.first().is_some_and(|p| p != "gh" && p != "git") {
            let program = inv.words.first()?.clone();
            let file = if program.contains('/') {
                path(inv.cwd.as_deref(), &program)
            } else {
                self.wrapper_paths.get(&program).cloned()
            };
            if let Some(file) = file.filter(|file| unchanged_before(file, at)) {
                let metadata = std::fs::metadata(&file).ok()?;
                if metadata.len() > 16 * 1024 {
                    return None;
                }
                let text = std::fs::read_to_string(&file).ok()?;
                if let Some(mut expanded) = wrapper(&text, inv.cwd.as_deref()) {
                    expanded.words.extend(inv.words.into_iter().skip(1));
                    inv = expanded;
                }
            }
        }
        let mut argv = inv.words;
        let program = argv.first()?.clone();
        argv.remove(0);
        if program == "git" {
            while argv.first().is_some_and(|w| w == "-C") {
                let dir = argv.get(1)?.clone();
                inv.cwd = path(inv.cwd.as_deref(), &dir);
                argv.drain(..2);
            }
            if argv.first().is_some_and(|w| w.starts_with('-')) {
                return None;
            }
        }
        let cwd = inv.cwd.clone();
        let mut evidence = cwd
            .as_ref()
            .and_then(|p| self.evidence.get(p))
            .cloned()
            .unwrap_or_default();
        if evidence.repo.is_none() {
            evidence.repo = remote(cwd.as_deref(), "origin", at);
        }
        let output = output.as_deref().unwrap_or("");
        // Read-only commands are evidence, not touches. State is local to this native
        // generation and invalidated by an unrecognized command, never guessed from HEAD.
        let read = match (program.as_str(), argv.as_slice()) {
            ("git", [a, b]) if a == "branch" && b == "--show-current" => {
                evidence.branch = branch(output.trim()).then(|| output.trim().to_owned());
                evidence.branch_at = at;
                true
            }
            ("git", [a, b]) if a == "rev-parse" && b == "HEAD" => {
                evidence.oid = oid(output.trim()).then(|| output.trim().to_owned());
                evidence.oid_at = at;
                true
            }
            ("git", [a, b, c]) if a == "remote" && b == "get-url" && c == "origin" => {
                evidence.repo = Repo::parse(output.trim(), &inv.host);
                true
            }
            ("command", [a, name]) if a == "-v" && branch(name) => {
                if let Some(file) = path(cwd.as_deref(), output.trim()) {
                    self.wrapper_paths.insert(name.clone(), file);
                }
                true
            }
            _ => false,
        };
        if read {
            if let Some(cwd) = cwd {
                self.evidence.insert(cwd, evidence);
            }
            return None;
        }
        let mut witnessed = evidence.clone();
        if started_at.is_none_or(|start| {
            witnessed.branch_at.is_none_or(|seen| seen > start)
                || witnessed.oid_at.is_none_or(|seen| seen > start)
        }) {
            witnessed.branch = None;
            witnessed.oid = None;
        }
        let result = if program == "gh" {
            gh_action(
                &argv,
                &inv.host,
                inv.repo.as_deref(),
                &witnessed,
                output,
                at,
            )
        } else if program == "git" {
            git_action(
                &argv,
                cwd.as_deref(),
                &witnessed,
                output,
                at,
                if cwd.as_deref() == base {
                    captured_head
                } else {
                    None
                },
            )
        } else {
            None
        };
        let harmless = (program == "gh"
            && argv.first().is_some_and(|a| a == "pr")
            && argv.get(1).is_some_and(|a| {
                matches!(
                    a.as_str(),
                    "view"
                        | "diff"
                        | "list"
                        | "checks"
                        | "review"
                        | "comment"
                        | "edit"
                        | "ready"
                        | "close"
                        | "reopen"
                        | "create"
                )
            }))
            || (program == "git"
                && matches!(
                    argv.first().map(String::as_str),
                    Some("status" | "diff" | "log" | "show" | "push")
                ));
        if program == "git"
            && matches!(
                argv.first().map(String::as_str),
                Some("checkout" | "switch")
            )
        {
            let positional = argv
                .iter()
                .skip(1)
                .filter(|w| !w.starts_with('-'))
                .collect::<Vec<_>>();
            evidence.branch = positional
                .last()
                .filter(|b| branch(b))
                .map(|b| b.trim_start_matches("origin/").to_owned());
            evidence.oid = receipt_oid(output);
            evidence.branch_at = at;
            evidence.oid_at = at;
        } else if !harmless
            || program == "gh"
                && argv
                    .get(1)
                    .is_some_and(|s| matches!(s.as_str(), "checkout" | "merge"))
        {
            evidence.branch = None;
            evidence.oid = None;
        }
        if let Some(cwd) = cwd {
            self.evidence.insert(cwd, evidence);
        }
        let (identity, lookup, action) = result?;
        // A credential binding with an explicit hostname cannot be reassigned to another host.
        let target_host = identity
            .as_ref()
            .map(|p| p.host.as_str())
            .or_else(|| lookup.as_ref().map(Lookup::host));
        if inv.account.is_some() && target_host.is_some_and(|host| host != inv.host) {
            return None;
        }
        Some(Action {
            identity,
            lookup,
            account: inv.account,
            action: action.into(),
            item: item_id.clone(),
            at,
        })
    }
}
impl Lookup {
    pub(super) fn host(&self) -> &str {
        match self {
            Self::Number(r, _) | Self::Run(r, ..) => &r.host,
            Self::Branch { head_repo, .. } => &head_repo.host,
        }
    }
}
type Identified = (Option<PrIdentity>, Option<Lookup>, &'static str);
fn gh_action(
    argv: &[String],
    host: &str,
    repo_env: Option<&str>,
    evidence: &Evidence,
    output: &str,
    at: Option<i64>,
) -> Option<Identified> {
    let mut args = Vec::new();
    let mut repo = repo_env.and_then(|r| Repo::parse(r, host));
    let mut i = 0;
    // -R is accepted in gh's root/subcommand positions. Body/title option values are
    // consumed below, so a quoted body containing '-R' never changes repository scope.
    while i < argv.len() {
        if argv[i] == "-R" || argv[i] == "--repo" {
            repo = Some(Repo::parse(argv.get(i + 1)?, host)?);
            i += 2;
        } else if let Some(r) = argv[i].strip_prefix("--repo=") {
            repo = Some(Repo::parse(r, host)?);
            i += 1;
        } else {
            let takes_value = [
                "--body",
                "-b",
                "--body-file",
                "-F",
                "--title",
                "-t",
                "--base",
                "-B",
                "--head",
                "-H",
                "--add-assignee",
                "--remove-assignee",
                "--add-reviewer",
                "--remove-reviewer",
                "--add-label",
                "--remove-label",
                "--add-project",
                "--remove-project",
                "--milestone",
                "--branch",
                "--match-head-commit",
                "--subject",
                "--interval",
                "-i",
                "--recover",
                "--json",
                "--jq",
                "-q",
                "--template",
                "--method",
                "-X",
                "--field",
                "--raw-field",
                "-f",
                "--input",
                "--header",
                "--hostname",
            ]
            .contains(&argv[i].as_str());
            args.push(argv[i].clone());
            i += 1;
            if takes_value {
                args.push(argv.get(i)?.clone());
                i += 1;
            }
        }
    }
    let explicit_repo = repo.clone();
    let repo = repo.or_else(|| evidence.repo.clone());
    if args.first()?.as_str() == "api" {
        return api_action(&args[1..], host, repo.as_ref(), output);
    }
    if args.first()?.as_str() == "run" {
        if args.get(1)?.as_str() != "watch" {
            return None;
        }
        let run = args.get(2)?.parse::<u64>().ok()?;
        if args
            .iter()
            .skip(3)
            .any(|a| !matches!(a.as_str(), "--exit-status" | "--compact"))
        {
            return None;
        }
        return Some((None, Some(Lookup::Run(repo?, run, at?)), "watched"));
    }
    let kind = args.first()?.as_str();
    let operation = args.get(1)?.as_str();
    let action = match (kind, operation) {
        ("pr", "create") => "opened",
        ("pr", "review") => "reviewed",
        ("pr", "comment") | ("issue", "comment") => "commented",
        ("pr", "edit" | "ready") => "updated",
        ("pr", "merge") => "merged",
        ("pr", "close") => "closed",
        ("pr", "reopen") => "reopened",
        ("pr", "checkout") => "checked out",
        ("pr", "checks") if args.iter().any(|a| a == "--watch") => "watched",
        _ => return None,
    };
    // `pr create` has no PR selector: its successful canonical receipt supplies identity.
    // Preserve existing discovery with creation-only options such as --reviewer/--assignee.
    if action == "opened" {
        return Some((
            Some(response(output, explicit_repo.as_ref(), host)?),
            None,
            action,
        ));
    }
    let mut selector = None;
    let mut i = 2;
    while i < args.len() {
        let word = &args[i];
        let name = word.split('=').next()?;
        if [
            "--body",
            "-b",
            "--body-file",
            "-F",
            "--title",
            "-t",
            "--base",
            "-B",
            "--head",
            "-H",
            "--add-assignee",
            "--remove-assignee",
            "--add-reviewer",
            "--remove-reviewer",
            "--add-label",
            "--remove-label",
            "--add-project",
            "--remove-project",
            "--milestone",
            "--branch",
            "--match-head-commit",
            "--subject",
            "--interval",
            "-i",
            "--recover",
            "--json",
            "--jq",
            "-q",
            "--template",
        ]
        .contains(&name)
        {
            if !word.contains('=') {
                i += 1;
                args.get(i)?;
            }
            i += 1;
            continue;
        }
        if [
            "--approve",
            "-a",
            "--request-changes",
            "-r",
            "--comment",
            "-c",
            "--auto",
            "--squash",
            "-s",
            "--merge",
            "-m",
            "--rebase",
            "--delete-branch",
            "-d",
            "--admin",
            "--draft",
            "--undo",
            "--force",
            "--recurse-submodules",
            "--detach",
            "--watch",
            "--fail-fast",
            "--required",
            "--edit-last",
            "--create-if-none",
            "--yes",
            "--web",
            "--fill",
            "--fill-first",
            "--fill-verbose",
            "--editor",
            "-e",
        ]
        .contains(&name)
        {
            i += 1;
            continue;
        }
        if word.starts_with('-') || selector.replace(word.as_str()).is_some() {
            return None;
        }
        i += 1;
    }
    let selected = selector.and_then(canonical);
    let receipt = response(
        output,
        selected
            .as_ref()
            .map(Repo::from_pr)
            .as_ref()
            .or(explicit_repo.as_ref()),
        host,
    );
    if let (Some(selected), Some(receipt)) = (&selected, &receipt)
        && selected != receipt
    {
        return None;
    }
    if let Some(selected) = selected {
        if explicit_repo
            .as_ref()
            .is_some_and(|r| *r != Repo::from_pr(&selected))
        {
            return None;
        }
        return Some((Some(selected), None, action));
    }
    if let Some(selector) = selector {
        if let Ok(number) = selector.parse::<u32>() {
            let r = repo?;
            if number == 0 {
                return None;
            }
            if let Some(receipt) = receipt {
                if receipt.number != number
                    || explicit_repo
                        .as_ref()
                        .is_some_and(|repo| *repo != Repo::from_pr(&receipt))
                {
                    return None;
                }
                return Some((Some(receipt), None, action));
            }
            // issue comments require a PR-only lookup; a bare number is never a ticket proof.
            return if kind == "issue" {
                Some((None, Some(Lookup::Number(r, number)), action))
            } else {
                Some((Some(r.pr(number)), None, action))
            };
        }
        if branch(selector) {
            let r = repo?;
            return Some((
                receipt,
                Some(Lookup::Branch {
                    repos: vec![r.clone()],
                    branch: selector.into(),
                    head_repo: r,
                    oid: String::new(),
                    at: at?,
                }),
                action,
            ));
        }
        return None;
    }
    if let Some(receipt) = receipt {
        return Some((Some(receipt), None, action));
    }
    if action == "opened" {
        return None;
    }
    let r = repo?;
    let branch = evidence.branch.clone()?;
    let oid = evidence.oid.clone()?;
    Some((
        None,
        Some(Lookup::Branch {
            repos: vec![r.clone()],
            branch,
            head_repo: r,
            oid,
            at: at?,
        }),
        action,
    ))
}
fn api_action(
    args: &[String],
    host: &str,
    _repo: Option<&Repo>,
    output: &str,
) -> Option<Identified> {
    let mut endpoint = None;
    let mut method = None;
    let mut fields = false;
    let mut host = host.to_owned();
    let mut i = 0;
    while i < args.len() {
        let word = &args[i];
        if matches!(word.as_str(), "-X" | "--method") {
            i += 1;
            method = Some(args.get(i)?.to_ascii_uppercase());
        } else if let Some(value) = word.strip_prefix("--method=") {
            method = Some(value.to_ascii_uppercase());
        } else if matches!(
            word.as_str(),
            "-f" | "--raw-field" | "-F" | "--field" | "--input"
        ) {
            fields = true;
            i += 1;
            args.get(i)?;
        } else if word.starts_with("--raw-field=")
            || word.starts_with("--field=")
            || word.starts_with("--input=")
        {
            fields = true;
        } else if word == "--hostname" {
            i += 1;
            host = args.get(i)?.clone();
        } else if matches!(
            word.as_str(),
            "-H" | "--header" | "--jq" | "-q" | "--template" | "-t"
        ) {
            i += 1;
            args.get(i)?;
        } else if matches!(
            word.as_str(),
            "--silent" | "--paginate" | "--slurp" | "--include" | "-i"
        ) {
        } else if word.starts_with('-') || endpoint.replace(word.as_str()).is_some() {
            return None;
        }
        i += 1;
    }
    let method = method.unwrap_or_else(|| if fields { "POST".into() } else { "GET".into() });
    if !matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE") {
        return None;
    }
    let mut endpoint = endpoint?;
    if let Some(rest) = endpoint.strip_prefix("https://api.github.com/") {
        host = "github.com".into();
        endpoint = rest;
    } else if let Some((h, rest)) = endpoint
        .strip_prefix("https://")
        .and_then(|url| url.split_once("/api/v3/"))
    {
        host = h.into();
        endpoint = rest;
    }
    let endpoint = endpoint.trim_start_matches('/');
    let parts = endpoint.split('/').collect::<Vec<_>>();
    if parts.len() < 4 || parts[0] != "repos" {
        return None;
    }
    let r = Repo::parse(&format!("{host}/{}/{}", parts[1], parts[2]), &host)?;
    if response(output, None, &host).is_some_and(|pr| Repo::from_pr(&pr) != r) {
        return None;
    }
    if parts.len() == 4 && parts[3] == "pulls" && method == "POST" {
        let receipt = response(output, Some(&r), &host).or_else(|| {
            serde_json::from_str::<Value>(output)
                .ok()
                .and_then(|v| v.as_u64().or_else(|| v["number"].as_u64()))
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
                .map(|n| r.pr(n))
        })?;
        return Some((Some(receipt), None, "opened"));
    }
    // Comment ids in the global edit route are not PR numbers. Only the canonical
    // mutation response can establish that relationship (a silent delete stays unknown).
    if matches!(
        parts.as_slice(),
        [_, _, _, "pulls" | "issues", "comments", _]
    ) && matches!(method.as_str(), "PATCH" | "DELETE")
    {
        return Some((Some(response(output, Some(&r), &host)?), None, "commented"));
    }
    let (r, n) = api_identity(endpoint, &host)?;
    let (action, confirmed) = match parts.as_slice() {
        [_, _, _, "pulls", _, "merge"] if method == "PUT" => ("merged", true),
        [_, _, _, "pulls", _, "reviews", ..] => ("reviewed", true),
        [_, _, _, "pulls", _, "comments", ..] => ("commented", true),
        [_, _, _, "issues", _, "comments", ..] if method == "POST" => ("commented", false),
        [_, _, _, "pulls", _, "requested_reviewers"] => ("updated", true),
        [_, _, _, "pulls", _] if method == "PATCH" => ("updated", true),
        [_, _, _, "issues", _] if method == "PATCH" => ("updated", false),
        _ => return None,
    };
    if let Some(receipt) = response(output, Some(&r), &host) {
        if receipt != r.pr(n) {
            return None;
        }
        return Some((Some(receipt), None, action));
    }
    if confirmed {
        Some((Some(r.pr(n)), None, action))
    } else {
        Some((None, Some(Lookup::Number(r, n)), action))
    }
}

fn receipt_oid(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("HEAD is now at ")
            .and_then(|tail| tail.split_whitespace().next())
            .filter(|s| oid(s))
            .map(str::to_owned)
    })
}
fn git_action(
    args: &[String],
    cwd: Option<&Path>,
    evidence: &Evidence,
    output: &str,
    at: Option<i64>,
    captured_head: Option<&str>,
) -> Option<Identified> {
    let operation = args.first()?.as_str();
    if operation == "push" {
        let mut positional = Vec::new();
        let mut i = 1;
        while i < args.len() {
            let word = &args[i];
            if [
                "--delete",
                "-d",
                "--mirror",
                "--all",
                "--tags",
                "--dry-run",
                "-n",
            ]
            .contains(&word.as_str())
            {
                return None;
            }
            if ["--repo", "--receive-pack", "--exec", "-o", "--push-option"]
                .contains(&word.as_str())
            {
                return None;
            }
            if word.starts_with('-') {
                if !matches!(
                    word.as_str(),
                    "-u" | "--set-upstream"
                        | "--force"
                        | "-f"
                        | "--force-with-lease"
                        | "--porcelain"
                        | "--verbose"
                        | "-v"
                        | "--quiet"
                        | "-q"
                        | "--no-verify"
                ) {
                    return None;
                }
            } else {
                positional.push(word.as_str());
            }
            i += 1;
        }
        if positional.len() > 2 {
            return None;
        }
        let remote_name = positional.first().copied().unwrap_or("origin");
        let pushed_repo = output
            .lines()
            .find_map(|line| {
                line.trim()
                    .strip_prefix("To ")
                    .and_then(|r| Repo::parse(r, "github.com"))
            })
            .or_else(|| Repo::parse(remote_name, "github.com"))
            .or_else(|| remote(cwd, remote_name, at))?;
        let mut updates = Vec::new();
        for line in output.lines() {
            let columns = line.split('\t').collect::<Vec<_>>();
            if columns.len() == 3 && !matches!(columns[0].trim(), "=" | "-" | "!") {
                if let Some((source, dest)) = columns[1].split_once(':')
                    && branch(source)
                    && branch(dest)
                {
                    let head = columns[2]
                        .split_whitespace()
                        .find_map(|word| {
                            word.split_once("..")
                                .map(|(_, new)| new.trim_start_matches('.'))
                                .filter(|s| oid(s))
                        })
                        .map(str::to_owned);
                    updates.push((source.to_owned(), dest.to_owned(), head));
                }
                continue;
            }
            if let Some((left, right)) = line.split_once(" -> ") {
                let destination = right.split_whitespace().next()?;
                let source = left.split_whitespace().last()?;
                if !branch(source) || !branch(destination) {
                    continue;
                }
                let pushed_oid = left.split_whitespace().find_map(|word| {
                    word.split_once("..")
                        .map(|(_, n)| n.trim_start_matches('.'))
                        .filter(|s| oid(s))
                });
                updates.push((
                    source.to_owned(),
                    destination.to_owned(),
                    pushed_oid.map(str::to_owned),
                ));
            }
        }
        if updates.len() != 1 {
            return None;
        }
        let (source, dest, pushed_oid) = updates.remove(0);
        let explicit = positional.get(1).map(|s| s.trim_start_matches('+'));
        if let Some(spec) = explicit {
            let (from, to) = spec.split_once(':').unwrap_or((spec, spec));
            if !branch(from) || !branch(to) {
                return None;
            }
            let to = to.trim_start_matches("refs/heads/");
            if to != dest.trim_start_matches("refs/heads/")
                || from != "HEAD"
                    && from.trim_start_matches("refs/heads/")
                        != source.trim_start_matches("refs/heads/")
            {
                return None;
            }
        }
        let head = pushed_oid.or_else(|| {
            let spec_source = explicit
                .and_then(|s| s.split(':').next())
                .unwrap_or(&source);
            let current = spec_source == "HEAD" || evidence.branch.as_deref() == Some(spec_source);
            current
                .then(|| {
                    captured_head
                        .filter(|s| oid(s))
                        .map(str::to_owned)
                        .or_else(|| evidence.oid.clone())
                })
                .flatten()
        })?;
        let branch = dest.trim_start_matches("refs/heads/").to_owned();
        let mut repos = vec![pushed_repo.clone()];
        for base in [evidence.repo.clone(), remote(cwd, "upstream", at)]
            .into_iter()
            .flatten()
        {
            if !repos.contains(&base) && base.host == pushed_repo.host {
                repos.push(base);
            }
        }
        return Some((
            None,
            Some(Lookup::Branch {
                repos,
                branch,
                head_repo: pushed_repo,
                oid: head,
                at: at?,
            }),
            "pushed",
        ));
    }
    let worktree = operation == "worktree" && args.get(1).is_some_and(|s| s == "add");
    if !worktree && !matches!(operation, "checkout" | "switch") {
        return None;
    }
    let mut pos = Vec::new();
    let mut i = if worktree { 2 } else { 1 };
    while i < args.len() {
        if args[i] == "--" || matches!(args[i].as_str(), "--orphan" | "--detach") {
            return None;
        }
        if matches!(args[i].as_str(), "-b" | "-B" | "-c" | "-C") {
            i += 1;
            args.get(i)?;
        } else if args[i].starts_with('-') {
            return None;
        } else {
            pos.push(args[i].as_str());
        }
        i += 1;
    }
    let target = if worktree {
        if pos.len() != 2 {
            return None;
        }
        pos[1]
    } else {
        if pos.len() != 1 {
            return None;
        }
        pos[0]
    };
    let target = target.strip_prefix("origin/").unwrap_or(target);
    if !branch(target) {
        return None;
    }
    let repo = evidence.repo.clone()?;
    let head = receipt_oid(output)?;
    Some((
        None,
        Some(Lookup::Branch {
            repos: vec![repo.clone()],
            branch: target.into(),
            head_repo: repo,
            oid: head,
            at: at?,
        }),
        "checked out",
    ))
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Checkpoint {
    pub source: String,
    pub identity: Option<PrIdentity>,
    pub retry_at: Option<i64>,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub account_until: i64,
}
pub(super) struct Resolution {
    pub identity: Option<PrIdentity>,
    pub retry_at: Option<i64>,
    pub account_until: i64,
}
const BRANCH_QUERY: &str = r#"query($owner:String!,$repo:String!,$branch:String!,$cursor:String){viewer{login} rateLimit{cost limit remaining resetAt} repository(owner:$owner,name:$repo){pullRequests(headRefName:$branch,states:[OPEN,CLOSED,MERGED],first:100,after:$cursor){totalCount pageInfo{hasNextPage endCursor} nodes{number url headRefName headRefOid createdAt closedAt headRepository{url nameWithOwner}}}}}"#;
const NUMBER_QUERY: &str = r#"query($owner:String!,$repo:String!,$number:Int!){viewer{login} rateLimit{cost limit remaining resetAt} repository(owner:$owner,name:$repo){pullRequest(number:$number){number url}}}"#;
fn primary_deadline(value: &Value, now: i64) -> i64 {
    let rate = &value["data"]["rateLimit"];
    let cost = rate["cost"].as_i64().unwrap_or(1).max(1);
    let remaining = rate["remaining"].as_i64().unwrap_or(0);
    let limit = rate["limit"].as_i64().unwrap_or(0);
    if remaining < cost.max((limit / 20).max(1)) {
        rate["resetAt"]
            .as_str()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_millisecond().max(now))
            .unwrap_or(now)
    } else {
        now
    }
}
async fn query(
    gh: &Gh,
    repo: &Repo,
    account: Option<&str>,
    query: &str,
    variables: Value,
    now: i64,
    budget: &mut u32,
) -> std::result::Result<(Value, i64), i64> {
    if *budget == 0 {
        return Err(now + 120_000);
    }
    *budget -= 1;
    let body = json!({"query":query,"variables":variables}).to_string();
    let output = gh
        .output_with_input(
            &[
                "api",
                "graphql",
                "--hostname",
                &repo.host,
                "--include",
                "--input",
                "-",
            ],
            &body,
        )
        .await
        .map_err(|_| now + 120_000)?;
    let (headers, payload) = super::split_headers(&output.stdout);
    let value = serde_json::from_str::<Value>(payload).unwrap_or(Value::Null);
    let until = primary_deadline(&value, now).max(super::retry_after(headers, now).unwrap_or(now));
    let not_pr = value
        .pointer("/data/repository")
        .is_some_and(Value::is_object)
        && value["errors"].as_array().is_some_and(|errors| {
            !errors.is_empty()
                && errors.iter().all(|error| {
                    error["type"] == "NOT_FOUND"
                        && error["path"]
                            .as_array()
                            .is_some_and(|path| path.iter().any(|part| part == "pullRequest"))
                })
        });
    if !output.success() || value.is_null() || value.get("errors").is_some() && !not_pr {
        return Err(until.max(now + 120_000));
    }
    if account.is_some_and(|account| {
        value
            .pointer("/data/viewer/login")
            .and_then(Value::as_str)
            .is_none_or(|viewer| !viewer.eq_ignore_ascii_case(account))
    }) {
        return Err(now + 120_000);
    }
    Ok((value, until))
}
fn at_time(pr: &Value, at: i64) -> bool {
    let time = |key: &str| {
        pr[key]
            .as_str()
            .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            .map(|t| t.as_millisecond())
    };
    time("createdAt").is_some_and(|created| created <= at)
        && time("closedAt").is_none_or(|closed| closed >= at)
}
pub(super) async fn resolve(gh: &Gh, action: &Action, now: i64, budget: &mut u32) -> Resolution {
    let none = || Resolution {
        identity: None,
        retry_at: None,
        account_until: now,
    };
    let retry = |until| Resolution {
        identity: None,
        retry_at: Some(until),
        account_until: until,
    };
    let Some(lookup) = &action.lookup else {
        return none();
    };
    match lookup {
        Lookup::Number(repo, number) => {
            let (value, until) = match query(
                gh,
                repo,
                action.account.as_deref(),
                NUMBER_QUERY,
                json!({"owner":repo.owner,"repo":repo.name,"number":number}),
                now,
                budget,
            )
            .await
            {
                Ok(value) => value,
                Err(until) => return retry(until),
            };
            let identity = value
                .pointer("/data/repository/pullRequest/url")
                .and_then(Value::as_str)
                .and_then(canonical)
                .filter(|pr| *pr == repo.pr(*number));
            Resolution {
                identity,
                retry_at: None,
                account_until: until,
            }
        }
        Lookup::Branch {
            repos,
            branch,
            head_repo,
            oid: head,
            at,
        } => {
            let mut identities = HashMap::new();
            let mut until = now;
            for repo in repos.iter().take(3) {
                let mut cursor = Value::Null;
                let mut seen = HashSet::new();
                let mut count = 0;
                for page in 0..4 {
                    let (value,deadline)=match query(gh,repo,action.account.as_deref(),BRANCH_QUERY,json!({"owner":repo.owner,"repo":repo.name,"branch":branch,"cursor":cursor}),now,budget).await{Ok(value)=>value,Err(until)=>return retry(until)};
                    until = until.max(deadline);
                    let Some(connection) = value.pointer("/data/repository/pullRequests") else {
                        return none();
                    };
                    let Some(nodes) = connection["nodes"].as_array() else {
                        return none();
                    };
                    count += nodes.len();
                    for pr in nodes {
                        let source = pr
                            .pointer("/headRepository/url")
                            .and_then(Value::as_str)
                            .and_then(|r| Repo::parse(r, &repo.host));
                        if source.as_ref() != Some(head_repo)
                            || pr["headRefName"] != branch.as_str()
                            || !at_time(pr, *at)
                        {
                            continue;
                        }
                        if !head.is_empty()
                            && !pr["headRefOid"]
                                .as_str()
                                .is_some_and(|h| h.starts_with(head))
                        {
                            continue;
                        }
                        if let Some(identity) = pr["url"]
                            .as_str()
                            .and_then(canonical)
                            .filter(|pr| Repo::from_pr(pr) == *repo)
                        {
                            identities.insert(identity.key(), identity);
                        }
                    }
                    match connection
                        .pointer("/pageInfo/hasNextPage")
                        .and_then(Value::as_bool)
                    {
                        Some(false) => {
                            if connection["totalCount"].as_u64() != Some(count as u64) {
                                return none();
                            }
                            break;
                        }
                        Some(true) => {
                            let Some(next) = connection
                                .pointer("/pageInfo/endCursor")
                                .and_then(Value::as_str)
                            else {
                                return none();
                            };
                            if page == 3 || !seen.insert(next.to_owned()) {
                                return none();
                            }
                            if until > now {
                                return retry(until);
                            }
                            cursor = json!(next);
                        }
                        None => return none(),
                    }
                }
                if until > now && repos.len() > 1 {
                    return retry(until);
                }
            }
            Resolution {
                identity: (identities.len() == 1).then(|| identities.into_values().next().unwrap()),
                retry_at: None,
                account_until: until,
            }
        }
        Lookup::Run(repo, run, at) => {
            if *budget == 0 {
                return retry(now + 120_000);
            }
            *budget -= 1;
            let endpoint = format!("repos/{}/{}/actions/runs/{run}", repo.owner, repo.name);
            let output = match gh
                .output(&[
                    "api",
                    "--method",
                    "GET",
                    "--hostname",
                    &repo.host,
                    "--include",
                    &endpoint,
                ])
                .await
            {
                Ok(output) => output,
                Err(_) => return retry(now + 120_000),
            };
            let (headers, body) = super::split_headers(&output.stdout);
            let until = super::retry_after(headers, now).unwrap_or(now);
            if !output.success() {
                return retry(until.max(now + 120_000));
            }
            let value = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
            if value["created_at"]
                .as_str()
                .and_then(|s| s.parse::<jiff::Timestamp>().ok())
                .is_none_or(|t| t.as_millisecond() > *at)
            {
                return none();
            }
            let Some(head) = value["head_sha"].as_str().filter(|s| oid(s)) else {
                return none();
            };
            let prs = value["pull_requests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|pr| pr.pointer("/head/sha").and_then(Value::as_str) == Some(head))
                .filter_map(|pr| {
                    pr["url"]
                        .as_str()
                        .and_then(|url| api_identity(url, &repo.host))
                        .map(|(r, n)| r.pr(n))
                })
                .collect::<Vec<_>>();
            let keys = prs.iter().map(PrIdentity::key).collect::<HashSet<_>>();
            Resolution {
                identity: (keys.len() == 1).then(|| prs[0].clone()),
                retry_at: None,
                account_until: until,
            }
        }
    }
}

/// Worker command-start HEAD is durable evidence only for an identical command/id with one
/// unique captured value. Reused native ids with conflicting captures remain unresolved.
pub(super) fn captured_heads(record: &Path) -> HashMap<(String, String), String> {
    let text = std::fs::read_to_string(record.join("commands.jsonl")).unwrap_or_default();
    let mut values: HashMap<(String, String), HashSet<String>> = HashMap::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v["line"] != "ran" {
            continue;
        }
        if let (Some(item), Some(command), Some(head)) = (
            v["item"].as_str(),
            v["command"].as_str(),
            v["head"].as_str().filter(|s| oid(s)),
        ) {
            values
                .entry((item.into(), command.into()))
                .or_default()
                .insert(head.into());
        }
    }
    let mut generations: HashMap<(String, String), HashSet<String>> = HashMap::new();
    for generation in super::super::session_dir::read_lineage(record) {
        let Some(file) = generation.file else {
            return HashMap::new();
        };
        let file = brigadier_providers::session_file::locate(generation.provider, Path::new(&file));
        let Ok(entries) = brigadier_providers::session_file::read(generation.provider, &file)
        else {
            return HashMap::new();
        };
        for entry in entries {
            if let ProviderEvent::Command {
                item_id, command, ..
            } = entry.event
            {
                generations
                    .entry((item_id, command))
                    .or_default()
                    .insert(generation.session_id.clone());
            }
        }
    }
    values.retain(|key, _| {
        generations
            .get(key)
            .is_some_and(|sessions| sessions.len() == 1)
    });
    values
        .into_iter()
        .filter_map(|(key, heads)| {
            (heads.len() == 1).then(|| (key, heads.into_iter().next().unwrap()))
        })
        .collect()
}

#[cfg(all(test, unix))]
mod tests;
