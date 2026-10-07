//! Commit message trailers: the Co-authored-by lines that name an AI, which the user's
//! setting leaves out of commits.

use std::{collections::HashMap, ffi::OsString};

use crate::{Oid, Repo, Result, TrailerStat, command::valid_oid, parse};

/// `message` without its Co-authored-by trailers that name an AI (people's are kept), and
/// without the blank lines that leaves at its end. Only the trailers at its end count: the
/// same line quoted in its body, with text after it, stays. Returned unchanged when it has
/// none, or when nothing else would be left.
pub fn strip_ai_coauthors(message: &str) -> String {
    let all: Vec<&str> = message.lines().collect();
    let mut end = all.len();
    while end > 0 && all[end - 1].trim().is_empty() {
        end -= 1;
    }
    // The trailer block: the `Key: value` lines that end the message, below its subject.
    let mut start = end;
    while start > 1 && trailer_line(all[start - 1]) {
        start -= 1;
    }
    if !all[start..end].iter().any(|line| ai_coauthor(line)) {
        return message.to_owned();
    }
    let mut lines = all[..start].to_vec();
    lines.extend(all[start..end].iter().filter(|line| !ai_coauthor(line)));
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return message.to_owned();
    }
    let mut cleaned = lines.join("\n");
    if message.ends_with('\n') {
        cleaned.push('\n');
    }
    cleaned
}

/// Whether `line` has a trailer's shape: a `Key: value` line, its key a token.
fn trailer_line(line: &str) -> bool {
    line.split_once(':').is_some_and(|(key, _)| {
        !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

/// The names of AI products and their companies.
fn ai_product(word: &str) -> bool {
    matches!(
        word,
        "claude"
            | "anthropic"
            | "codex"
            | "openai"
            | "copilot"
            | "gemini"
            | "cursoragent"
            | "devin"
            | "aider"
    ) || word.starts_with("gpt")
        || word.ends_with("gpt")
}

/// The other words an AI's co-author name is made of: its model, its edition, its maker.
fn ai_name_word(word: &str) -> bool {
    ai_product(word)
        || word.bytes().all(|b| b.is_ascii_digit())
        || matches!(
            word,
            "opus"
                | "sonnet"
                | "haiku"
                | "fable"
                | "code"
                | "cli"
                | "agent"
                | "ai"
                | "assistant"
                | "bot"
                | "swe"
                | "integration"
                | "cursor"
                | "github"
                | "google"
                | "mini"
                | "pro"
                | "flash"
        )
}

fn words(text: &str) -> Vec<&str> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect()
}

/// Whether `line` is a Co-authored-by trailer that names an AI: its address is an AI's (a
/// product's no-reply or bot address), or its name is wholly an AI's ("Claude Opus 5",
/// "GitHub Copilot"). A person who shares a word with one ("Claude Martin") is kept.
fn ai_coauthor(line: &str) -> bool {
    let Some((key, value)) = line.split_once(':') else {
        return false;
    };
    if !key.trim().eq_ignore_ascii_case("co-authored-by") {
        return false;
    }
    let value = value.to_ascii_lowercase();
    let (name, email) = match value.split_once('<') {
        Some((name, rest)) => (name, rest.split_once('>').map_or(rest, |(email, _)| email)),
        None => (value.as_str(), ""),
    };
    let name = words(name);
    // One word alone ("Claude", "Devin") is a person's name too: then the address decides.
    let whole_name = name.len() > 1 || email.trim().is_empty();
    if whole_name
        && name.iter().all(|word| ai_name_word(word))
        && name.iter().any(|word| ai_product(word))
    {
        return true;
    }
    let (local, domain) = email.trim().rsplit_once('@').unwrap_or(("", ""));
    let robot = local.contains("noreply") || local.contains("no-reply") || local.contains("[bot]");
    match domain {
        "anthropic.com" | "openai.com" | "google.com" | "github.com" | "aider.chat" => {
            robot || words(local).iter().any(|word| ai_product(word))
        }
        "cursor.com" => local == "cursoragent",
        // A GitHub account: an AI's app ("…[bot]"), or Copilot's own.
        "users.noreply.github.com" => {
            let login = local.rsplit_once('+').map_or(local, |(_, login)| login);
            login == "copilot" || (robot && words(login).iter().any(|word| ai_product(word)))
        }
        _ => robot && words(local).iter().any(|word| ai_product(word)),
    }
}

impl Repo {
    /// Whether a commit from `base` (excluded) to `tip` carries the trailer `key: value`, as
    /// git parses a message's trailers (`%(trailers:key=…,valueonly)`; the value compared
    /// without case).
    pub fn has_trailer(&self, base: &Oid, tip: &Oid, key: &str, value: &str) -> Result<bool> {
        valid_oid(base)?;
        valid_oid(tip)?;
        let format = format!("--format=%(trailers:key={key},valueonly)");
        let range = format!("{}..{}", base.0, tip.0);
        let out = self.cmd(&["log", &format, &range, "--"], true)?;
        Ok(parse::text(&out)?
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case(value)))
    }

    /// The commits from `base` (excluded) to `tip` that carry the trailer `key: value` (as
    /// [`Self::has_trailer`] matches it), and the lines they added and removed (`--numstat`;
    /// merges show none, binary files count none).
    pub fn trailer_stat(
        &self,
        base: &Oid,
        tip: &Oid,
        key: &str,
        value: &str,
    ) -> Result<TrailerStat> {
        valid_oid(base)?;
        valid_oid(tip)?;
        // Each commit starts with a record separator, then its trailer values on one line.
        let format = format!("--format=%x1e%(trailers:key={key},valueonly,separator=%x1f)");
        let range = format!("{}..{}", base.0, tip.0);
        let out = self.cmd(
            &["log", "--no-renames", "--numstat", &format, &range, "--"],
            true,
        )?;
        let mut stat = TrailerStat::default();
        for commit in parse::text(&out)?.split('\x1e').skip(1) {
            let (values, numstat) = commit.split_once('\n').unwrap_or((commit, ""));
            if !values
                .split('\x1f')
                .any(|found| found.trim().eq_ignore_ascii_case(value))
            {
                continue;
            }
            stat.commits += 1;
            for line in numstat.lines() {
                let mut fields = line.splitn(3, '\t');
                let (Some(added), Some(removed), Some(_)) =
                    (fields.next(), fields.next(), fields.next())
                else {
                    continue;
                };
                // A binary file shows `-` for both.
                stat.added += added.parse::<u64>().unwrap_or(0);
                stat.removed += removed.parse::<u64>().unwrap_or(0);
            }
        }
        Ok(stat)
    }

    /// The commits from `base` (excluded) to `tip`, with the Co-authored-by trailers that name
    /// an AI left out of their messages: `None`, with nothing written, when no message has one.
    /// Otherwise every commit from the first such message on is written again with the same
    /// tree, the same author and committer (names, emails and dates), and its parents
    /// replaced by their rewritten commits, merges included; returns the new tip. No ref
    /// moves. `None` too for anything this can't keep exactly: `tip` not after `base`, a
    /// message in another encoding, a merge tag.
    pub fn clean_ai_coauthors(&self, base: &Oid, tip: &Oid) -> Result<Option<Oid>> {
        valid_oid(base)?;
        valid_oid(tip)?;
        if base == tip || !self.ancestor(base, tip)? {
            return Ok(None);
        }
        let range = format!("{}..{}", base.0, tip.0);
        let list = self.cmd(&["rev-list", "--reverse", "--topo-order", &range], true)?;
        let mut commits = Vec::new();
        let mut any = false;
        for line in parse::text(&list)?.lines() {
            let commit = parse::oid(line.as_bytes())?;
            let Some(raw) = self.raw_commit(&commit)? else {
                return Ok(None);
            };
            any |= strip_ai_coauthors(&raw.message) != raw.message;
            commits.push((commit, raw));
        }
        if !any {
            return Ok(None);
        }
        let mut rewritten: HashMap<Oid, Oid> = HashMap::new();
        for (commit, raw) in commits {
            let parents: Vec<Oid> = raw
                .parents
                .iter()
                .map(|parent| rewritten.get(parent).unwrap_or(parent).clone())
                .collect();
            let message = strip_ai_coauthors(&raw.message);
            if message == raw.message && parents == raw.parents {
                continue;
            }
            let parents: Vec<&Oid> = parents.iter().collect();
            let new = self.commit_tree_with(&raw.tree, &parents, &message, &raw.idents)?;
            rewritten.insert(commit, new);
        }
        Ok(rewritten.get(tip).cloned())
    }

    /// A commit's tree, parents, exact message, and author and committer as git environment;
    /// `None` when a header couldn't be written back the same (a signature is dropped).
    fn raw_commit(&self, commit: &Oid) -> Result<Option<RawCommit>> {
        let raw = self.cmd(&["cat-file", "commit", &commit.0], true)?;
        let raw = parse::text(&raw)?;
        let (headers, message) = raw.split_once("\n\n").unwrap_or((raw, ""));
        let mut tree = None;
        let mut parents = Vec::new();
        let mut idents = Vec::new();
        for header in headers.lines() {
            // A continuation of a multi-line header (a signature).
            if header.starts_with(' ') {
                continue;
            }
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "tree" => tree = Some(parse::oid(value.as_bytes())?),
                "parent" => parents.push(parse::oid(value.as_bytes())?),
                "author" | "committer" => {
                    let Some((ident, date)) = value.rsplit_once('>') else {
                        return Ok(None);
                    };
                    let Some((name, email)) = ident.split_once('<') else {
                        return Ok(None);
                    };
                    let role = key.to_ascii_uppercase();
                    idents.extend([
                        (format!("GIT_{role}_NAME"), name.trim_end()),
                        (format!("GIT_{role}_EMAIL"), email),
                        (format!("GIT_{role}_DATE"), date.trim()),
                    ]);
                }
                "gpgsig" | "gpgsig-sha256" => {}
                _ => return Ok(None),
            }
        }
        let (Some(tree), 6) = (tree, idents.len()) else {
            return Ok(None);
        };
        Ok(Some(RawCommit {
            tree,
            parents,
            message: message.to_owned(),
            idents: idents
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        }))
    }
}

/// A commit as [`Repo::clean_ai_coauthors`] writes it again.
struct RawCommit {
    tree: Oid,
    parents: Vec<Oid>,
    message: String,
    idents: Vec<(OsString, OsString)>,
}
