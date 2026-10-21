//! The stored MCP sign-ins: Claude Code's credentials item (or its `.credentials.json`), Codex's
//! Keychain items or `.credentials.json` (`mcp_oauth_credentials_store`), and Brigadier's own
//! Keychain item (docs/research/phase7-probes.md (j)).
//!
//! Claude's item also holds the user's claude.ai login and API key: a read deserializes only the
//! one `mcpOAuth` entry asked for (and its configured client secret), skipping every other field
//! unread, and the item's bytes are wiped as soon as it is parsed. Only a renewal's write-back
//! holds the whole item, as a JSON value that is written back with nothing changed but the
//! entry's three token fields.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brigadier_providers::cli::CliEnv;
use brigadier_providers::login_mark::{claude_keychain_account, claude_keychain_service};
use brigadier_registry::{
    CodexStore, OAuthClient, Owner, Roots, Secret, Server, Transport, claude_key, claude_lock_name,
    codex_keys, identity, sha256_hex,
};
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use super::keychain::{Keychain, StoreError};
use super::locks::{self, Held};

/// Brigadier's own sign-ins: one item per server, its account the hash of its identity.
pub(crate) const OWN_SERVICE: &str = "Brigadier MCP sign-ins";
const CODEX_SERVICE: &str = "Codex MCP Credentials";

/// Milliseconds since the epoch.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Where one stored sign-in for a server is kept. Brigadier's own places are equal by
/// identity alone: every spelling of a server's address shares its one Keychain item, so it
/// shares what is cached of it, its renewals and its sign-out.
#[derive(Debug, Clone)]
pub(crate) enum Place {
    /// Claude Code's entry under its key.
    Claude { key: String, url: String },
    /// Codex's sign-in to the server `name` at `url`, in the store `store` names.
    Codex {
        name: String,
        url: String,
        keys: Vec<String>,
        store: CodexStore,
    },
    /// Brigadier's own, by the server's identity.
    Brigadier { identity: String, url: String },
}

impl PartialEq for Place {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Claude { key, url }, Self::Claude { key: k, url: u }) => key == k && url == u,
            (
                Self::Codex {
                    name,
                    url,
                    keys,
                    store,
                },
                Self::Codex {
                    name: n,
                    url: u,
                    keys: k,
                    store: s,
                },
            ) => name == n && url == u && keys == k && store == s,
            (Self::Brigadier { identity, .. }, Self::Brigadier { identity: i, .. }) => {
                identity == i
            }
            _ => false,
        }
    }
}

impl Eq for Place {}

impl Hash for Place {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Claude { key, url } => (key, url).hash(state),
            Self::Codex {
                name,
                url,
                keys,
                store,
            } => (name, url, keys, store).hash(state),
            Self::Brigadier { identity, .. } => identity.hash(state),
        }
    }
}

impl Place {
    /// Where `owner` keeps its sign-in to `server`; `None` for a server that isn't remote.
    pub(crate) fn of(owner: Owner, server: &Server, codex_store: CodexStore) -> Option<Self> {
        let Transport::Http { url, .. } = &server.transport else {
            return None;
        };
        let url = url.clone();
        Some(match owner {
            Owner::Claude => Self::Claude {
                key: claude_key(server)?,
                url,
            },
            Owner::Codex => Self::Codex {
                name: server
                    .name
                    .strip_prefix("local:")
                    .unwrap_or(&server.name)
                    .to_owned(),
                keys: codex_keys(server),
                url,
                store: codex_store,
            },
            Owner::Brigadier => Self::Brigadier {
                identity: identity(&server.transport)?,
                url,
            },
        })
    }

    /// Who keeps it, for logs.
    pub(crate) fn owner(&self) -> &'static str {
        match self {
            Self::Claude { .. } => "Claude Code",
            Self::Codex { .. } => "Codex",
            Self::Brigadier { .. } => "Brigadier",
        }
    }
}

/// Where a credential was read from, so its renewal goes back there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    ClaudeItem,
    ClaudeFile,
    /// A Codex Keychain item, by its account.
    CodexItem(String),
    /// An entry of Codex's file, by its key.
    CodexFile(String),
    Own,
}

/// A stored sign-in. Its `Debug` shows no value.
#[derive(Clone)]
pub(crate) struct Credential {
    pub access_token: Secret,
    pub refresh_token: Option<Secret>,
    /// When the access token expires (ms since the epoch); `None`: not said.
    pub expires_at: Option<u64>,
    pub client_id: Option<String>,
    pub client_secret: Option<Secret>,
    /// The authorization server's token endpoint, when the store has it.
    pub token_endpoint: Option<String>,
    /// When the user signed in (ms since the epoch): Brigadier's own sign-ins only.
    pub since: Option<u64>,
    pub found: Found,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credential")
            .field("expires_at", &self.expires_at)
            .field("since", &self.since)
            .field("found", &self.found)
            .finish_non_exhaustive()
    }
}

impl Credential {
    /// It can serve: an access token not yet expired, or a refresh token.
    pub(crate) fn usable(&self) -> bool {
        self.renewable()
            || (!self.access_token.expose().is_empty() && !self.expires_within(Duration::ZERO))
    }

    /// Its access token is due for renewal: expired, or expiring within `margin` when a refresh
    /// token can renew it early. One without a refresh token serves until it expires.
    pub(crate) fn due(&self, margin: Duration) -> bool {
        self.expires_within(Duration::ZERO) || (self.renewable() && self.expires_within(margin))
    }

    fn renewable(&self) -> bool {
        self.refresh_token
            .as_ref()
            .is_some_and(|token| !token.expose().trim().is_empty())
    }

    /// It holds the same tokens as `other`.
    pub(crate) fn same_tokens(&self, other: &Self) -> bool {
        self.access_token == other.access_token && self.refresh_token == other.refresh_token
    }

    /// Its access token expires within `margin` (or has).
    pub(crate) fn expires_within(&self, margin: Duration) -> bool {
        let margin = u64::try_from(margin.as_millis()).unwrap_or(u64::MAX);
        self.expires_at
            .is_some_and(|at| now_ms().saturating_add(margin) >= at)
    }

    /// The configured client of the server's config, where the store has none.
    pub(crate) fn with_client(mut self, client: Option<&OAuthClient>) -> Self {
        if let Some(client) = client {
            if self.client_id.is_none() {
                self.client_id.clone_from(&client.client_id);
            }
            if self.client_secret.is_none() {
                self.client_secret.clone_from(&client.client_secret);
            }
        }
        self
    }
}

/// Where the CLIs keep their stores on this machine.
#[derive(Debug, Clone)]
pub(crate) struct Homes {
    /// Claude Code's credentials item (service, account): on macOS.
    pub claude_item: Option<(String, String)>,
    /// Claude Code's config folder, which holds `.credentials.json`.
    pub claude_dir: PathBuf,
    /// Where Claude Code keeps its locks (`CLAUDE_SECURESTORAGE_CONFIG_DIR`, else its config
    /// folder).
    pub claude_locks: PathBuf,
    pub codex_home: PathBuf,
}

impl Homes {
    pub(crate) fn new(roots: &Roots) -> Self {
        let env = CliEnv::from_vars(
            roots
                .vars
                .iter()
                .map(|(name, value)| (name.into(), value.into())),
        );
        let claude_item = cfg!(target_os = "macos")
            .then(|| claude_keychain_service(&env, &roots.claude_dir))
            .flatten()
            .map(|service| (service, claude_keychain_account(env.var("USER"))));
        let claude_locks = roots
            .vars
            .get("CLAUDE_SECURESTORAGE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map_or_else(|| roots.claude_dir.clone(), PathBuf::from);
        Self {
            claude_item,
            claude_dir: roots.claude_dir.clone(),
            claude_locks,
            codex_home: roots.codex_home.clone(),
        }
    }

    fn claude_file(&self) -> PathBuf {
        self.claude_dir.join(".credentials.json")
    }

    fn codex_file(&self) -> PathBuf {
        self.codex_home.join(".credentials.json")
    }

    fn codex_lock(&self, name: &str) -> PathBuf {
        self.codex_home.join("mcp-oauth-locks").join(name)
    }
}

/// The stores, read and written through `keychain`.
pub(crate) struct Stores {
    pub keychain: Arc<dyn Keychain>,
    pub homes: Homes,
}

fn failed(what: &str, err: impl fmt::Display) -> StoreError {
    StoreError::Failed(format!("{what}: {err}"))
}

/// Held while Brigadier's own sign-ins are written or deleted, so a renewal can't put back
/// one signed out of meanwhile.
static OWN_WRITES: Mutex<()> = Mutex::new(());

/// A store's bytes as read, and where they were found.
type ReadItem = (Zeroizing<Vec<u8>>, Found);

/// A file's bytes, `None` when it isn't there.
fn read_file(path: &Path) -> Result<Option<Zeroizing<Vec<u8>>>, StoreError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(failed("reading a credentials file", err)),
    }
}

/// Replaces `path` with `bytes`, readable by the user alone.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let dir = path
        .parent()
        .ok_or_else(|| StoreError::Failed("a credentials file has no folder".into()))?;
    let partial = dir.join(format!(".brigadier-{}", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&partial)?, bytes)?;
        std::fs::rename(&partial, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    written.map_err(|err| failed("writing a credentials file", err))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeEntry {
    server_url: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_at: Option<f64>,
    client_id: Option<String>,
    client_secret: Option<String>,
    discovery_state: Option<DiscoveryState>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryState {
    authorization_server_metadata: Option<AuthorizationServer>,
}

#[derive(Deserialize)]
struct AuthorizationServer {
    token_endpoint: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeClientConfig {
    client_secret: Option<String>,
}

/// The value under the first of `keys` a JSON object has, every other value skipped unread.
struct Pick<'a, T> {
    keys: &'a [String],
    of: PhantomData<T>,
}

impl<'de, T: Deserialize<'de>> DeserializeSeed<'de> for Pick<'_, T> {
    type Value = Option<(String, T)>;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de, T: Deserialize<'de>> Visitor<'de> for Pick<'_, T> {
    type Value = Option<(String, T)>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut found = None;
        while let Some(key) = map.next_key::<String>()? {
            if found.is_none() && self.keys.contains(&key) {
                found = Some((key, map.next_value::<T>()?));
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(found)
    }
}

/// Claude's item, read for one key: its `mcpOAuth` entry and its configured client secret.
struct ClaudeBlob<'a> {
    keys: &'a [String],
}

type ClaudePicked = (Option<ClaudeEntry>, Option<ClaudeClientConfig>);

impl<'de> DeserializeSeed<'de> for ClaudeBlob<'_> {
    type Value = ClaudePicked;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ClaudeBlob<'_> {
    type Value = ClaudePicked;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let (mut entry, mut config) = (None, None);
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "mcpOAuth" => {
                    entry = map
                        .next_value_seed(Pick {
                            keys: self.keys,
                            of: PhantomData,
                        })?
                        .map(|(_, entry)| entry);
                }
                "mcpOAuthClientConfig" => {
                    config = map
                        .next_value_seed(Pick {
                            keys: self.keys,
                            of: PhantomData,
                        })?
                        .map(|(_, config)| config);
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok((entry, config))
    }
}

/// Codex's Keychain value (`StoredOAuthTokens`).
#[derive(Deserialize)]
struct CodexItem {
    server_name: String,
    url: String,
    client_id: Option<String>,
    token_response: CodexTokens,
    expires_at: Option<u64>,
}

#[derive(Deserialize)]
struct CodexTokens {
    access_token: String,
    refresh_token: Option<String>,
}

/// An entry of Codex's `.credentials.json`.
#[derive(Deserialize)]
struct CodexFileEntry {
    server_name: String,
    server_url: String,
    client_id: Option<String>,
    access_token: String,
    expires_at: Option<u64>,
    refresh_token: Option<String>,
    #[serde(default)]
    executor_owned: bool,
}

/// Brigadier's own item.
#[derive(Serialize, Deserialize)]
struct Own {
    url: String,
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
    client_id: Option<String>,
    client_secret: Option<String>,
    token_endpoint: Option<String>,
    /// When the user signed in (ms since the epoch).
    #[serde(default)]
    since: Option<u64>,
}

fn secret(value: Option<String>) -> Option<Secret> {
    value.filter(|value| !value.is_empty()).map(Secret::new)
}

impl Stores {
    /// The sign-in `place` holds; `None` when it holds none for the server. A `quiet` read
    /// never shows the system's prompt.
    pub(crate) fn read(
        &self,
        place: &Place,
        quiet: bool,
    ) -> Result<Option<Credential>, StoreError> {
        match place {
            Place::Claude { key, url } => self.read_claude(key, url, quiet),
            Place::Codex {
                name,
                url,
                keys,
                store,
            } => self.read_codex(name, url, keys, *store, quiet),
            // Its account is the server's identity: any spelling of its address matches.
            Place::Brigadier { identity, .. } => {
                let Some(bytes) = self
                    .keychain
                    .get(OWN_SERVICE, &sha256_hex(identity), quiet)?
                else {
                    return Ok(None);
                };
                let own: Own = serde_json::from_slice(&bytes)
                    .map_err(|err| failed("Brigadier's sign-in", err))?;
                Ok(Some(Credential {
                    access_token: Secret::new(own.access_token),
                    refresh_token: secret(own.refresh_token),
                    expires_at: own.expires_at,
                    client_id: own.client_id,
                    client_secret: secret(own.client_secret),
                    token_endpoint: own.token_endpoint,
                    since: own.since,
                    found: Found::Own,
                }))
            }
        }
    }

    /// Claude's whole item (or file), as read: `None` when it has neither.
    fn claude_bytes(&self, quiet: bool) -> Result<Option<ReadItem>, StoreError> {
        if let Some((service, account)) = &self.homes.claude_item
            && let Some(bytes) = self.keychain.get(service, account, quiet)?
        {
            return Ok(Some((bytes, Found::ClaudeItem)));
        }
        Ok(read_file(&self.homes.claude_file())?.map(|bytes| (bytes, Found::ClaudeFile)))
    }

    fn read_claude(
        &self,
        key: &str,
        url: &str,
        quiet: bool,
    ) -> Result<Option<Credential>, StoreError> {
        let Some((bytes, found)) = self.claude_bytes(quiet)? else {
            return Ok(None);
        };
        let keys = [key.to_owned()];
        let (entry, config) = ClaudeBlob { keys: &keys }
            .deserialize(&mut serde_json::Deserializer::from_slice(&bytes))
            .map_err(|err| failed("Claude Code's credentials", err))?;
        drop(bytes);
        let Some(entry) = entry.filter(|entry| entry.server_url.as_deref() == Some(url)) else {
            return Ok(None);
        };
        let Some(access_token) = entry.access_token.filter(|token| !token.is_empty()) else {
            return Ok(None);
        };
        Ok(Some(Credential {
            access_token: Secret::new(access_token),
            refresh_token: secret(entry.refresh_token),
            expires_at: entry
                .expires_at
                .filter(|at| at.is_finite() && *at >= 0.0)
                .map(|at| at as u64),
            client_id: entry.client_id,
            client_secret: secret(entry.client_secret)
                .or_else(|| secret(config.and_then(|config| config.client_secret))),
            token_endpoint: entry
                .discovery_state
                .and_then(|state| state.authorization_server_metadata)
                .and_then(|metadata| metadata.token_endpoint),
            since: None,
            found,
        }))
    }

    /// Whether Codex reads `store` from its Keychain and from its file, as it does: the
    /// Keychain first by default. Off macOS its keyring is one Brigadier doesn't read.
    fn codex_backends(store: CodexStore) -> Result<(bool, bool), StoreError> {
        let keychain = cfg!(target_os = "macos");
        match store {
            CodexStore::Auto => Ok((keychain, true)),
            CodexStore::File => Ok((false, true)),
            CodexStore::Keyring if keychain => Ok((true, false)),
            CodexStore::Keyring => Err(StoreError::Unsupported),
            CodexStore::Other => Err(StoreError::Failed(
                "Codex keeps its sign-ins where Brigadier doesn't read".into(),
            )),
        }
    }

    fn read_codex(
        &self,
        name: &str,
        url: &str,
        keys: &[String],
        store: CodexStore,
        quiet: bool,
    ) -> Result<Option<Credential>, StoreError> {
        let (keychain, file) = Self::codex_backends(store)?;
        if keychain {
            let mut failure = None;
            for key in keys {
                // Its attributes first: a server Codex isn't signed in to asks nothing.
                match self.keychain.exists(CODEX_SERVICE, key) {
                    Ok(false) => continue,
                    Ok(true) => {}
                    Err(err) => {
                        failure = Some(err);
                        continue;
                    }
                }
                let Some(bytes) = self.keychain.get(CODEX_SERVICE, key, quiet)? else {
                    continue;
                };
                let item: CodexItem =
                    serde_json::from_slice(&bytes).map_err(|err| failed("Codex's sign-in", err))?;
                drop(bytes);
                if item.url != url || item.server_name != name {
                    continue;
                }
                return Ok(Some(Credential {
                    access_token: Secret::new(item.token_response.access_token),
                    refresh_token: secret(item.token_response.refresh_token),
                    expires_at: item.expires_at,
                    client_id: item.client_id,
                    client_secret: None,
                    token_endpoint: None,
                    since: None,
                    found: Found::CodexItem(key.clone()),
                }));
            }
            // Codex falls back to its file only when the Keychain had nothing or failed.
            if let (Some(err), false) = (failure, file) {
                return Err(err);
            }
        }
        if !file {
            return Ok(None);
        }
        // Off macOS Codex may keep it in a keyring Brigadier doesn't read, before its file.
        let unread = || {
            if store == CodexStore::Auto && !keychain {
                Err(StoreError::Unsupported)
            } else {
                Ok(None)
            }
        };
        let _shared = locks::codex(&self.homes.codex_lock("file-store.lock"), true)?;
        let Some(bytes) = read_file(&self.homes.codex_file())? else {
            return unread();
        };
        let picked = Pick::<CodexFileEntry> {
            keys,
            of: PhantomData,
        }
        .deserialize(&mut serde_json::Deserializer::from_slice(&bytes))
        .map_err(|err| failed("Codex's credentials file", err))?;
        drop(bytes);
        let Some((key, entry)) = picked.filter(|(_, entry)| {
            !entry.executor_owned && entry.server_url == url && entry.server_name == name
        }) else {
            return unread();
        };
        Ok(Some(Credential {
            access_token: Secret::new(entry.access_token),
            refresh_token: secret(entry.refresh_token),
            expires_at: entry.expires_at,
            client_id: entry.client_id,
            client_secret: None,
            token_endpoint: None,
            since: None,
            found: Found::CodexFile(key),
        }))
    }

    /// The owner's lock around renewing `place`'s sign-in, as the owning CLI takes it.
    pub(crate) fn renewal_lock(&self, place: &Place) -> Result<Vec<Held>, StoreError> {
        match place {
            Place::Claude { key, .. } => Ok(vec![locks::claude(
                &self.homes.claude_locks.join(claude_lock_name(key)),
                Duration::from_secs(60),
            )?]),
            // Either key may be the one this Codex build hashes: both are taken, in order.
            Place::Codex { keys, .. } => {
                let mut names: Vec<String> = keys
                    .iter()
                    .map(|key| format!("{}.lock", sha256_hex(key)))
                    .collect();
                names.sort();
                names.dedup();
                names
                    .iter()
                    .map(|name| locks::codex(&self.homes.codex_lock(name), false))
                    .collect()
            }
            Place::Brigadier { .. } => Ok(Vec::new()),
        }
    }

    /// Writes `renewed` back where `old` was read, changing nothing else there, then reads it
    /// back to check. The caller holds [`Self::renewal_lock`]. Fails, writing nothing, when the
    /// entry is gone, belongs to another server, or no longer holds `old`'s tokens.
    pub(crate) fn write_back(
        &self,
        place: &Place,
        old: &Credential,
        renewed: &Credential,
    ) -> Result<(), StoreError> {
        let unchanged = |access: Option<&Value>, refresh: Option<&Value>| {
            access.and_then(Value::as_str) == Some(old.access_token.expose())
                && refresh.and_then(Value::as_str) == old.refresh_token.as_ref().map(Secret::expose)
        };
        let gone = || StoreError::Failed("the stored sign-in changed or is gone".into());
        let new_access = || Value::String(renewed.access_token.expose().to_owned());
        let new_refresh = || {
            renewed.refresh_token.as_ref().map_or(Value::Null, |token| {
                Value::String(token.expose().to_owned())
            })
        };
        match (place, &old.found) {
            (Place::Claude { key, url }, found @ (Found::ClaudeItem | Found::ClaudeFile)) => {
                let _writing = locks::claude(
                    &self.homes.claude_locks.join(".storage-write"),
                    Duration::from_secs(15),
                )?;
                let (bytes, now_found) = self.claude_bytes(false)?.ok_or_else(gone)?;
                if now_found != *found {
                    return Err(gone());
                }
                let mut blob: Value = serde_json::from_slice(&bytes)
                    .map_err(|err| failed("Claude Code's credentials", err))?;
                drop(bytes);
                let entry = blob
                    .get_mut("mcpOAuth")
                    .and_then(|all| all.get_mut(key.as_str()))
                    .and_then(Value::as_object_mut)
                    .ok_or_else(gone)?;
                if entry.get("serverUrl").and_then(Value::as_str) != Some(url)
                    || !unchanged(entry.get("accessToken"), entry.get("refreshToken"))
                {
                    return Err(gone());
                }
                entry.insert("accessToken".into(), new_access());
                if renewed.refresh_token.is_some() {
                    entry.insert("refreshToken".into(), new_refresh());
                }
                match renewed.expires_at {
                    Some(at) => entry.insert("expiresAt".into(), Value::from(at)),
                    None => entry.remove("expiresAt"),
                };
                let bytes = Zeroizing::new(
                    serde_json::to_vec(&blob).map_err(|err| failed("Claude's credentials", err))?,
                );
                drop(blob);
                match found {
                    Found::ClaudeItem => {
                        let (service, account) =
                            self.homes.claude_item.as_ref().ok_or_else(gone)?;
                        self.keychain.update(service, account, &bytes)?;
                    }
                    _ => write_file(&self.homes.claude_file(), &bytes)?,
                }
            }
            (Place::Codex { name, url, .. }, Found::CodexItem(account)) => {
                let bytes = self
                    .keychain
                    .get(CODEX_SERVICE, account, false)?
                    .ok_or_else(gone)?;
                let mut item: Value =
                    serde_json::from_slice(&bytes).map_err(|err| failed("Codex's sign-in", err))?;
                drop(bytes);
                if item.get("url").and_then(Value::as_str) != Some(url)
                    || item.get("server_name").and_then(Value::as_str) != Some(name)
                {
                    return Err(gone());
                }
                let tokens = item
                    .get_mut("token_response")
                    .and_then(Value::as_object_mut)
                    .ok_or_else(gone)?;
                if !unchanged(tokens.get("access_token"), tokens.get("refresh_token")) {
                    return Err(gone());
                }
                tokens.insert("access_token".into(), new_access());
                if renewed.refresh_token.is_some() {
                    tokens.insert("refresh_token".into(), new_refresh());
                }
                // Codex works the remaining time out from `expires_at` when it reads.
                tokens.remove("expires_in");
                item["expires_at"] = renewed.expires_at.map_or(Value::Null, Value::from);
                let bytes = Zeroizing::new(
                    serde_json::to_vec(&item).map_err(|err| failed("Codex's sign-in", err))?,
                );
                drop(item);
                self.keychain.update(CODEX_SERVICE, account, &bytes)?;
            }
            (Place::Codex { name, url, .. }, Found::CodexFile(key)) => {
                let _writing = locks::codex(&self.homes.codex_lock("file-store.lock"), false)?;
                let bytes = read_file(&self.homes.codex_file())?.ok_or_else(gone)?;
                let mut file: Value = serde_json::from_slice(&bytes)
                    .map_err(|err| failed("Codex's credentials file", err))?;
                drop(bytes);
                let entry = file
                    .get_mut(key.as_str())
                    .and_then(Value::as_object_mut)
                    .ok_or_else(gone)?;
                if entry.get("server_url").and_then(Value::as_str) != Some(url)
                    || entry.get("server_name").and_then(Value::as_str) != Some(name)
                    || !unchanged(entry.get("access_token"), entry.get("refresh_token"))
                {
                    return Err(gone());
                }
                entry.insert("access_token".into(), new_access());
                if renewed.refresh_token.is_some() {
                    entry.insert("refresh_token".into(), new_refresh());
                }
                entry.insert(
                    "expires_at".into(),
                    renewed.expires_at.map_or(Value::Null, Value::from),
                );
                let bytes = Zeroizing::new(
                    serde_json::to_vec(&file).map_err(|err| failed("Codex's credentials", err))?,
                );
                drop(file);
                write_file(&self.homes.codex_file(), &bytes)?;
            }
            (Place::Brigadier { identity, url }, Found::Own) => {
                let _writing = OWN_WRITES.lock().unwrap_or_else(PoisonError::into_inner);
                match self.read(place, false)? {
                    Some(now)
                        if now.access_token == old.access_token
                            && now.refresh_token == old.refresh_token => {}
                    _ => return Err(gone()),
                }
                self.put_own(identity, url, renewed)?;
            }
            _ => return Err(gone()),
        }
        // Read back: the store must now hold the new token.
        match self.read(place, false)? {
            Some(now) if now.access_token == renewed.access_token => Ok(()),
            _ => Err(StoreError::Failed(
                "the renewed sign-in didn't read back".into(),
            )),
        }
    }

    /// Stores Brigadier's own sign-in to the server `identity` names.
    pub(crate) fn save_own(
        &self,
        identity: &str,
        url: &str,
        credential: &Credential,
    ) -> Result<(), StoreError> {
        let _writing = OWN_WRITES.lock().unwrap_or_else(PoisonError::into_inner);
        self.put_own(identity, url, credential)
    }

    fn put_own(
        &self,
        identity: &str,
        url: &str,
        credential: &Credential,
    ) -> Result<(), StoreError> {
        let own = Own {
            url: url.to_owned(),
            access_token: credential.access_token.expose().to_owned(),
            refresh_token: credential
                .refresh_token
                .as_ref()
                .map(|token| token.expose().to_owned()),
            expires_at: credential.expires_at,
            client_id: credential.client_id.clone(),
            client_secret: credential
                .client_secret
                .as_ref()
                .map(|secret| secret.expose().to_owned()),
            token_endpoint: credential.token_endpoint.clone(),
            since: credential.since,
        };
        let bytes = Zeroizing::new(
            serde_json::to_vec(&own).map_err(|err| failed("Brigadier's sign-in", err))?,
        );
        self.keychain
            .set(OWN_SERVICE, &sha256_hex(identity), &bytes)
    }

    /// Deletes Brigadier's own sign-in to the server `identity` names, if it has one.
    pub(crate) fn remove_own(&self, identity: &str) -> Result<(), StoreError> {
        let _writing = OWN_WRITES.lock().unwrap_or_else(PoisonError::into_inner);
        self.keychain.remove_own(&sha256_hex(identity))
    }
}
