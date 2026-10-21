//! Which stored sign-in can serve a remote server (docs/research/phase7-probes.md (j)): each
//! CLI's own key for an entry of its config, and the identity that matches one server across
//! configs. Reading the stores is the caller's ([`SignInStores`]): this crate never touches a
//! credential.

use sha2::{Digest, Sha256};

use crate::{Agent, Server, Source, Transport};

/// Who keeps a stored sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    Claude,
    Codex,
    /// Brigadier's own, from a sign-in in Brigadier: keyed by [`identity`].
    Brigadier,
}

impl From<Agent> for Owner {
    fn from(agent: Agent) -> Self {
        match agent {
            Agent::Claude => Self::Claude,
            Agent::Codex => Self::Codex,
        }
    }
}

/// What a store holds for a server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// A sign-in read and found usable: an access token not yet expired, or a refresh token.
    Usable,
    /// None, or one that can't serve any more.
    Absent,
    /// Not known: the store couldn't be read without asking the user, they said no, or it is
    /// one Brigadier doesn't read.
    Unknown,
}

/// The stored sign-ins, as far as the caller can read them.
pub trait SignInStores {
    /// What `owner` holds for `server`: one of `owner`'s own entries for Claude and Codex
    /// (their keys are the entry's), any entry for Brigadier.
    fn probe(&self, owner: Owner, server: &Server) -> Probe;
}

/// Stores that are never read: every probe is [`Probe::Unknown`].
pub struct Unread;

impl SignInStores for Unread {
    fn probe(&self, _: Owner, _: &Server) -> Probe {
        Probe::Unknown
    }
}

/// The agent whose own config `source` is.
pub fn owner_agent(source: Source) -> Option<Agent> {
    match source {
        Source::ClaudeUser | Source::ClaudeLocal | Source::ClaudeProject => Some(Agent::Claude),
        Source::CodexUser | Source::CodexProject => Some(Agent::Codex),
        Source::Desktop | Source::CursorUser | Source::CursorProject => None,
    }
}

/// One remote server across configs: its address with the scheme and host in lower case, the
/// default port and a trailing slash dropped (its path and query kept as written), and its
/// headers other than `Authorization`, names in lower case and values exact. Two entries with
/// the same identity reach the same server as the same tenant. `None` for a stdio server.
pub fn identity(transport: &Transport) -> Option<String> {
    let Transport::Http { url, headers } = transport else {
        return None;
    };
    let mut headers: Vec<(String, &str)> = headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.expose()))
        .collect();
    headers.sort_unstable();
    let mut text = normal_url(url);
    for (name, value) in headers {
        text.push('\n');
        text.push_str(&name);
        text.push(':');
        text.push_str(value);
    }
    Some(text)
}

/// `url` with its scheme and host in lower case, no default port, no fragment and no trailing
/// slash on its path.
fn normal_url(url: &str) -> String {
    let url = url.split_once('#').map_or(url, |(url, _)| url);
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let scheme = scheme.to_ascii_lowercase();
    let end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let mut host = authority.to_ascii_lowercase();
    let default = match scheme.as_str() {
        "http" => ":80",
        "https" => ":443",
        _ => "",
    };
    if !default.is_empty() && host.ends_with(default) {
        host.truncate(host.len() - default.len());
    }
    let (path, query) = tail
        .split_once('?')
        .map_or((tail, None), |(p, q)| (p, Some(q)));
    let path = path.trim_end_matches('/');
    match query {
        Some(query) => format!("{scheme}://{host}{path}?{query}"),
        None => format!("{scheme}://{host}{path}"),
    }
}

/// Claude Code's key for `server`'s sign-in, `<name>|<first 16 hex of sha256(JSON
/// {type,url,headers})>` (Claude Code 2.1.306: `JSON.stringify({type, url, headers:
/// headers||{}})`, keys in that order and the headers as configured).
pub fn claude_key(server: &Server) -> Option<String> {
    let Transport::Http { url, headers } = &server.transport else {
        return None;
    };
    let headers: Vec<String> = headers
        .iter()
        .map(|(name, value)| format!("{}:{}", json_string(name), json_string(value.expose())))
        .collect();
    let payload = format!(
        r#"{{"type":"http","url":{},"headers":{{{}}}}}"#,
        json_string(url),
        headers.join(",")
    );
    Some(format!("{}|{}", server.name, sha256_prefix(&payload)))
}

/// The folder name of Claude Code's refresh lock for `key`: every character but an ASCII
/// letter or digit becomes `_`.
pub fn claude_lock_name(key: &str) -> String {
    let key: String = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("mcp-refresh-{key}.lock")
}

/// Codex's keys for `server`'s sign-in, `<name>|<first 16 hex of sha256(JSON {"type":"http",
/// "url", "headers":{}})>` (codex-rs 0.163.0 `compute_store_key`, the URL as configured and no
/// headers). The JSON's key order depends on how the binary was built (serde_json's
/// `preserve_order`), so both orders are given.
pub fn codex_keys(server: &Server) -> Vec<String> {
    let Transport::Http { url, .. } = &server.transport else {
        return Vec::new();
    };
    let name = server.name.strip_prefix("local:").unwrap_or(&server.name);
    let url = json_string(url);
    [
        format!(r#"{{"type":"http","url":{url},"headers":{{}}}}"#),
        format!(r#"{{"headers":{{}},"type":"http","url":{url}}}"#),
    ]
    .iter()
    .map(|payload| format!("{name}|{}", sha256_prefix(payload)))
    .collect()
}

/// The hex of `text`'s SHA-256, whole.
pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn sha256_prefix(text: &str) -> String {
    let mut hex = sha256_hex(text);
    hex.truncate(16);
    hex
}

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// The entries that could hold another stored sign-in for `server`: every Claude or Codex entry
/// with its identity, `server` itself first.
pub(crate) fn same_server<'a>(
    servers: &'a [Server],
    server: &'a Server,
) -> impl Iterator<Item = &'a Server> + 'a {
    let id = identity(&server.transport);
    std::iter::once(server).chain(servers.iter().filter(move |other| {
        other.id != server.id && other.signs_in && id.is_some() && identity(&other.transport) == id
    }))
}
