//! Share an agent's MCP OAuth sign-ins between Brigadier-managed accounts. Default homes
//! are read-only sources. Only MCP entries are merged, under the CLI's credential-store lock;
//! provider logins and other secrets are never carried. Payloads have no Debug implementation
//! and errors never contain credential data.
//!
//! Claude Code 2.1.311 keeps `mcpOAuth` beside `claudeAiOauth`, in its per-config Keychain
//! item or `.credentials.json`. Codex 0.164.0 uses a globally shared direct Keychain entry
//! (already shared), or a home-local fallback file / `secrets/mcp_oauth.age`. The latter is
//! decrypted and re-encrypted with each home's existing key; the other namespaces stay put.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, mpsc};
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{ProviderKind, accounts, cli::CliEnv, login_mark};

type Entries = BTreeMap<String, Value>;

// Decrypt only after ciphertext or key changes. Scrypt is deliberately expensive; polling
// must not repeat it for every idle account. Cache entries hold no additional disk copies.
struct Decrypted {
    ciphertext: String,
    key: String,
    value: Value,
}
static DECRYPTED: LazyLock<Mutex<BTreeMap<PathBuf, Decrypted>>> = LazyLock::new(Mutex::default);

/// Sync current sign-ins, including refreshes made by running CLIs. `homes` must contain only
/// managed account homes, never the main home. Busy stores are retried on the next pass.
/// Production credential access is deliberately excluded from test builds.
pub fn sync_accounts(kind: ProviderKind, env: &CliEnv, homes: &[PathBuf]) -> io::Result<()> {
    #[cfg(not(test))]
    {
        sync_with(kind, env, homes, &SystemKeyring)
    }
    #[cfg(test)]
    {
        let _ = (kind, env, homes);
        Ok(())
    }
}

/// Remove only this resource's MCP grants from managed accounts after the CLI's logout.
/// The caller serializes this with `sync_accounts`; the default home remains read-only.
pub fn forget_resource(
    kind: ProviderKind,
    env: &CliEnv,
    homes: &[PathBuf],
    resource: &str,
) -> io::Result<()> {
    #[cfg(not(test))]
    {
        forget_with(kind, env, homes, resource, &SystemKeyring)
    }
    #[cfg(test)]
    {
        let _ = (kind, env, homes, resource);
        Ok(())
    }
}

trait Keyring {
    fn get(&self, service: &str, account: &str) -> io::Result<Option<Vec<u8>>>;
    fn set(&self, service: &str, account: &str, data: &[u8]) -> io::Result<()>;
    /// Creates a missing encryption key, returning an existing one if another CLI won.
    fn insert(&self, service: &str, account: &str, data: &[u8]) -> io::Result<Vec<u8>>;
}

#[cfg(not(test))]
struct SystemKeyring;

#[cfg(all(not(test), target_os = "macos"))]
impl Keyring for SystemKeyring {
    fn get(&self, service: &str, account: &str) -> io::Result<Option<Vec<u8>>> {
        match security_framework::passwords::get_generic_password(service, account) {
            Ok(data) => Ok(Some(data)),
            Err(error) if error.code() == -25300 => Ok(None),
            Err(error) => Err(io::Error::other(format!(
                "MCP Keychain read failed ({})",
                error.code()
            ))),
        }
    }
    fn set(&self, service: &str, account: &str, data: &[u8]) -> io::Result<()> {
        security_framework::passwords::set_generic_password(service, account, data).map_err(
            |error| io::Error::other(format!("MCP Keychain write failed ({})", error.code())),
        )
    }
    fn insert(&self, service: &str, account: &str, data: &[u8]) -> io::Result<Vec<u8>> {
        let keychain = security_framework::os::macos::keychain::SecKeychain::default()
            .map_err(|_| io::Error::other("MCP encryption key store unavailable"))?;
        match keychain.add_generic_password(service, account, data) {
            Ok(()) => Ok(data.to_vec()),
            Err(error) if error.code() == -25299 => self
                .get(service, account)?
                .ok_or_else(|| io::Error::other("MCP encryption key disappeared")),
            Err(error) => Err(io::Error::other(format!(
                "MCP encryption key creation failed ({})",
                error.code()
            ))),
        }
    }
}

#[cfg(all(not(test), not(target_os = "macos")))]
impl Keyring for SystemKeyring {
    fn get(&self, _service: &str, _account: &str) -> io::Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn set(&self, _service: &str, _account: &str, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::other("MCP Keychain is unavailable"))
    }
    fn insert(&self, _service: &str, _account: &str, _data: &[u8]) -> io::Result<Vec<u8>> {
        Err(io::Error::other("MCP Keychain is unavailable"))
    }
}

#[derive(Clone, Copy)]
enum Format {
    Claude,
    CodexFile,
    CodexEncrypted,
}

struct Store {
    home: PathBuf,
    format: Format,
    keychain: Option<(String, String)>,
}

impl Store {
    fn file(&self) -> PathBuf {
        self.home.join(match self.format {
            Format::Claude | Format::CodexFile => ".credentials.json",
            Format::CodexEncrypted => "secrets/mcp_oauth.age",
        })
    }

    fn read(&self, keyring: &impl Keyring) -> io::Result<Value> {
        if let Some((service, account)) = &self.keychain
            && let Some(bytes) = keyring.get(service, account)?
        {
            return object(&bytes);
        }
        let Some(bytes) = read_file(&self.file())? else {
            return Ok(json!({}));
        };
        match self.format {
            Format::CodexEncrypted => {
                let passphrase = self
                    .passphrase(keyring)?
                    .ok_or_else(|| io::Error::other("MCP encrypted store has no accessible key"))?;
                let ciphertext = hash(&bytes);
                let key = hash(passphrase.as_bytes());
                {
                    let cache = DECRYPTED.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(cached) = cache.get(&self.file())
                        && cached.ciphertext == ciphertext
                        && cached.key == key
                    {
                        return Ok(cached.value.clone());
                    }
                }
                let identity = age::scrypt::Identity::new(passphrase.into());
                let plain = age::decrypt(&identity, &bytes)
                    .map_err(|_| io::Error::other("MCP store decryption failed"))?;
                let value = object(&plain)?;
                if value
                    .get("version")
                    .and_then(Value::as_u64)
                    .is_none_or(|v| v > 1)
                {
                    return Err(io::Error::other("unsupported MCP encrypted store version"));
                }
                let mut cache = DECRYPTED.lock().unwrap_or_else(|p| p.into_inner());
                if cache.len() >= 128 {
                    cache.clear();
                }
                cache.insert(
                    self.file(),
                    Decrypted {
                        ciphertext,
                        key,
                        value: value.clone(),
                    },
                );
                Ok(value)
            }
            _ => object(&bytes),
        }
    }

    fn passphrase(&self, keyring: &impl Keyring) -> io::Result<Option<String>> {
        keyring
            .get("codex", &self.key_account()?)?
            .map(|bytes| {
                String::from_utf8(bytes).map_err(|_| io::Error::other("invalid MCP store key"))
            })
            .transpose()
    }

    fn key_account(&self) -> io::Result<String> {
        let path = fs::canonicalize(&self.home)?;
        let digest = hash(path.to_string_lossy().as_bytes());
        Ok(format!("secrets|{}", &digest[..16]))
    }

    fn write_passphrase(&self, keyring: &impl Keyring) -> io::Result<String> {
        if let Some(key) = self.passphrase(keyring)? {
            return Ok(key);
        }
        // A missing key must never make existing encrypted account data unreadable.
        if [
            "mcp_oauth.age",
            "codex_auth.age",
            "local.age",
            "gateway_oauth.age",
        ]
        .iter()
        .any(|name| self.home.join("secrets").join(name).exists())
        {
            return Err(io::Error::other(
                "existing encrypted store has no accessible key",
            ));
        }
        let generated = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let key = keyring.insert("codex", &self.key_account()?, generated.as_bytes())?;
        String::from_utf8(key).map_err(|_| io::Error::other("invalid MCP store key"))
    }

    fn write(&self, value: &Value, keyring: &impl Keyring) -> io::Result<()> {
        let bytes =
            serde_json::to_vec(value).map_err(|_| io::Error::other("MCP serialization failed"))?;
        if let Some((service, account)) = &self.keychain {
            // Keep an existing plaintext fallback in that store. Do not move account-login
            // fields between stores or downgrade a Keychain credential to a plaintext file.
            if keyring.get(service, account)?.is_some() || !self.file().exists() {
                return keyring.set(service, account, &bytes);
            }
        }
        let bytes = if matches!(self.format, Format::CodexEncrypted) {
            let passphrase = self.write_passphrase(keyring)?;
            let mut recipient = age::scrypt::Recipient::new(passphrase.into());
            #[cfg(test)]
            recipient.set_work_factor(10);
            #[cfg(not(test))]
            let _ = &mut recipient;
            age::encrypt(&recipient, &bytes)
                .map_err(|_| io::Error::other("MCP store encryption failed"))?
        } else {
            bytes
        };
        if matches!(self.format, Format::CodexEncrypted) {
            accounts::create_private_dir(&self.home.join("secrets"))?;
        }
        write_private(&self.file(), &bytes)
    }

    fn may_create_file(&self) -> bool {
        if !matches!(self.format, Format::CodexFile) || self.file().exists() {
            return true;
        }
        let config = codex_config(&self.home);
        let mode = config
            .as_ref()
            .and_then(|c| c.get("mcp_oauth_credentials_store"))
            .and_then(toml_edit::Item::as_str);
        let backend = config
            .as_ref()
            .and_then(|c| c.get("auth_keyring_backend"))
            .and_then(toml_edit::Item::as_str);
        // Never export encrypted/keyring-only sign-ins into a new plaintext fallback.
        mode == Some("file")
            || (mode != Some("keyring")
                && backend != Some("secrets")
                && !self.home.join("secrets/mcp_oauth.age").exists())
    }

    fn entries(&self, value: &Value) -> Entries {
        let map = match self.format {
            Format::Claude => value.get("mcpOAuth"),
            Format::CodexFile => Some(value),
            Format::CodexEncrypted => value.get("secrets"),
        }
        .and_then(Value::as_object);
        map.into_iter()
            .flatten()
            .filter_map(|(key, value)| {
                let entry = match self.format {
                    Format::CodexEncrypted => {
                        if !key.starts_with("global/MCP_OAUTH_") {
                            return None;
                        }
                        serde_json::from_str::<Value>(value.as_str()?).ok()?
                    }
                    _ => value.clone(),
                };
                let key = match self.format {
                    Format::CodexEncrypted => codex_key(&entry)?,
                    _ => key.clone(),
                };
                valid_entry(self.format, &entry).then_some((key, entry))
            })
            .collect()
    }

    fn merge(&self, mut value: Value, entries: &Entries) -> Value {
        let old = self.entries(&value);
        for (key, entry) in entries {
            if !newer(self.format, entry, old.get(key)) {
                continue;
            }
            match self.format {
                Format::Claude => {
                    let map = value.as_object_mut().expect("validated object");
                    let target = map.entry("mcpOAuth").or_insert_with(|| json!({}));
                    // Unexpected schemas are not replaced with an empty store.
                    if let Some(target) = target.as_object_mut() {
                        target.insert(key.clone(), entry.clone());
                    }
                }
                Format::CodexFile => {
                    value
                        .as_object_mut()
                        .expect("validated object")
                        .insert(key.clone(), codex_file_entry(entry));
                }
                Format::CodexEncrypted => {
                    let map = value.as_object_mut().expect("validated object");
                    map.entry("version").or_insert(json!(1));
                    let target = map.entry("secrets").or_insert_with(|| json!({}));
                    if let Some(target) = target.as_object_mut() {
                        let hash = hash(key.as_bytes()).to_uppercase();
                        target.insert(
                            format!("global/MCP_OAUTH_{}", &hash[..32]),
                            Value::String(codex_stored_entry(entry).to_string()),
                        );
                    }
                }
            }
        }
        value
    }

    fn lock(&self) -> io::Result<Option<StoreLock>> {
        owned_path(&self.home, &self.file())?;
        match self.format {
            Format::Claude => {
                let path = self.home.join(".storage-write.lock");
                if let Ok(metadata) = fs::symlink_metadata(&path)
                    && metadata.is_dir()
                    && metadata
                        .modified()?
                        .elapsed()
                        .is_ok_and(|age| age > Duration::from_secs(15))
                {
                    // The same lease timeout Claude uses. Active holders heartbeat the dir.
                    match fs::remove_dir(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(error) => return Err(error),
                    }
                }
                match fs::create_dir(&path) {
                    Ok(()) => DirectoryLock::new(path)
                        .map(|lock| Some(StoreLock::Directory(Box::new(lock)))),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(None),
                    Err(error) => Err(error),
                }
            }
            Format::CodexFile | Format::CodexEncrypted => {
                let directory = self.home.join("mcp-oauth-locks");
                owned_path(&self.home, &directory)?;
                accounts::create_private_dir(&directory)?;
                let path = directory.join(match self.format {
                    Format::CodexFile => "file-store.lock",
                    _ => "secrets-store.lock",
                });
                owned_path(&self.home, &path)?;
                let file = private_options()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(path)?;
                match file.try_lock() {
                    Ok(()) => Ok(Some(StoreLock::File(file))),
                    Err(std::fs::TryLockError::WouldBlock) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            }
        }
    }
}

enum StoreLock {
    Directory(Box<DirectoryLock>),
    File(File),
}
impl Drop for StoreLock {
    fn drop(&mut self) {
        match self {
            Self::Directory(_) => {}
            Self::File(file) => {
                let _ = file.unlock();
            }
        }
    }
}

// Claude's proper-lockfile lease becomes stale after 15 seconds. Keep its mtime alive even
// while the OS credential service is waiting, and never remove a replacement lock on drop.
struct DirectoryLock {
    path: PathBuf,
    metadata: fs::Metadata,
    stop: Option<mpsc::Sender<()>>,
    heartbeat: Option<std::thread::JoinHandle<()>>,
}
impl DirectoryLock {
    fn new(path: PathBuf) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(0x02000000); // FILE_FLAG_BACKUP_SEMANTICS: open a directory.
        }
        let file = options.open(&path)?;
        let metadata = file.metadata()?;
        let (stop, receiver) = mpsc::channel();
        let heartbeat = std::thread::spawn(move || {
            while receiver.recv_timeout(Duration::from_secs(2))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                if file.set_modified(SystemTime::now()).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            path,
            metadata,
            stop: Some(stop),
            heartbeat: Some(heartbeat),
        })
    }
    fn check(&self) -> io::Result<()> {
        let current = fs::metadata(&self.path)?;
        #[cfg(unix)]
        let same = {
            use std::os::unix::fs::MetadataExt;
            current.dev() == self.metadata.dev() && current.ino() == self.metadata.ino()
        };
        #[cfg(not(unix))]
        let same = current.created().ok() == self.metadata.created().ok();
        if same {
            Ok(())
        } else {
            Err(io::Error::other("MCP credential lock changed"))
        }
    }
}
impl Drop for DirectoryLock {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(heartbeat) = self.heartbeat.take() {
            let _ = heartbeat.join();
        }
        if self.check().is_ok() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

fn sync_with(
    kind: ProviderKind,
    env: &CliEnv,
    homes: &[PathBuf],
    keyring: &impl Keyring,
) -> io::Result<()> {
    let Some(main) = accounts::main_home(kind, env) else {
        return Ok(());
    };
    let main_real = fs::canonicalize(&main).ok();
    let homes: Vec<_> = homes
        .iter()
        .filter(|home| {
            home.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("acct-"))
                && fs::symlink_metadata(home)
                    .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                && fs::canonicalize(home).ok() != main_real
        })
        .collect();
    if homes.is_empty() {
        return Ok(());
    }
    let source = stores(kind, env, &main);
    let mut destinations = Vec::new();
    for home in &homes {
        for store in stores(kind, &env.for_account(kind, home), home) {
            // Fail closed if a CLI environment override redirects a managed credential
            // store into the user's default home. The main home is read-only here.
            if store.home != **home
                || source.iter().any(|main| {
                    main.home == store.home
                        || (store.keychain.is_some() && main.keychain == store.keychain)
                })
            {
                return Err(io::Error::other(
                    "MCP account store aliases the default home",
                ));
            }
            destinations.push(store);
        }
    }
    let mut entries = Entries::new();
    let mut error = None;
    for store in source.iter().chain(&destinations) {
        // No locks or directory creation in the default home: it is strictly read-only.
        let result = store.read(keyring);
        match result {
            Ok(value) => {
                for (key, entry) in store.entries(&value) {
                    if newer(store.format, &entry, entries.get(&key)) {
                        entries.insert(key, entry);
                    }
                }
            }
            Err(err) => {
                error = Some(err);
            }
        }
    }
    for store in &destinations {
        if !store.may_create_file() {
            continue;
        }
        let result = (|| {
            let Some(lock) = store.lock()? else {
                return Ok(());
            };
            let before = store.read(keyring)?;
            let after = store.merge(before.clone(), &entries);
            if before != after {
                if let StoreLock::Directory(lock) = &lock {
                    lock.check()?;
                }
                store.write(&after, keyring)?;
            }
            Ok(())
        })();
        if let Err(err) = result {
            error = Some(err);
        }
    }
    error.map_or(Ok(()), Err)
}

fn forget_with(
    kind: ProviderKind,
    env: &CliEnv,
    homes: &[PathBuf],
    resource: &str,
    keyring: &impl Keyring,
) -> io::Result<()> {
    let Some(main) = accounts::main_home(kind, env) else {
        return Ok(());
    };
    let main_real = fs::canonicalize(&main).ok();
    let source = stores(kind, env, &main);
    for home in homes {
        if !home
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("acct-"))
            || !fs::symlink_metadata(home)
                .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
            || fs::canonicalize(home).ok() == main_real
        {
            continue;
        }
        for store in stores(kind, &env.for_account(kind, home), home) {
            if store.home != *home
                || source.iter().any(|main| {
                    main.home == store.home
                        || (store.keychain.is_some() && main.keychain == store.keychain)
                })
            {
                return Err(io::Error::other(
                    "MCP account store aliases the default home",
                ));
            }
            let Some(_lock) = store.lock()? else {
                return Err(io::Error::other(
                    "MCP account store is busy; retry sign-out",
                ));
            };
            let before = store.read(keyring)?;
            let mut after = before.clone();
            let target = match store.format {
                Format::Claude => after.get_mut("mcpOAuth"),
                Format::CodexFile => Some(&mut after),
                Format::CodexEncrypted => after.get_mut("secrets"),
            }
            .and_then(Value::as_object_mut);
            if let Some(target) = target {
                target.retain(|key, value| {
                    let parsed;
                    let entry = if matches!(store.format, Format::CodexEncrypted) {
                        if !key.starts_with("global/MCP_OAUTH_") {
                            return true;
                        }
                        parsed = value
                            .as_str()
                            .and_then(|text| serde_json::from_str::<Value>(text).ok());
                        let Some(entry) = parsed.as_ref() else {
                            return true;
                        };
                        entry
                    } else {
                        value
                    };
                    !valid_entry(store.format, entry)
                        || entry
                            .get("serverUrl")
                            .or_else(|| entry.get("server_url"))
                            .or_else(|| entry.get("url"))
                            .and_then(Value::as_str)
                            != Some(resource)
                });
            }
            if after != before {
                store.write(&after, keyring)?;
            }
        }
    }
    Ok(())
}

fn stores(kind: ProviderKind, env: &CliEnv, home: &Path) -> Vec<Store> {
    match kind {
        ProviderKind::Claude => {
            let directory = env
                .var("CLAUDE_SECURESTORAGE_CONFIG_DIR")
                .map(|value| {
                    if value.is_empty() {
                        env.home()
                            .unwrap_or_else(|| home.to_owned())
                            .join(".claude")
                    } else {
                        PathBuf::from(value)
                    }
                })
                .unwrap_or_else(|| home.to_owned());
            let keychain = cfg!(target_os = "macos")
                .then(|| {
                    login_mark::claude_keychain_service(env, home).map(|service| {
                        (
                            service,
                            login_mark::claude_keychain_account(env.var("USER")),
                        )
                    })
                })
                .flatten();
            vec![Store {
                home: directory,
                format: Format::Claude,
                keychain,
            }]
        }
        ProviderKind::Codex => {
            let mut out = vec![Store {
                home: home.to_owned(),
                format: Format::CodexFile,
                keychain: None,
            }];
            // The encrypted namespace has its own key and store lock. Never copy the key,
            // codex_auth.age, or the rest of secrets/. Missing keys are created atomically;
            // existing keys are never replaced.
            let config = codex_config(home);
            let encrypted = config.as_ref().is_some_and(|config| {
                config
                    .get("auth_keyring_backend")
                    .and_then(toml_edit::Item::as_str)
                    == Some("secrets")
                    && config
                        .get("mcp_oauth_credentials_store")
                        .and_then(toml_edit::Item::as_str)
                        != Some("file")
            });
            if encrypted || home.join("secrets/mcp_oauth.age").is_file() {
                out.push(Store {
                    home: home.to_owned(),
                    format: Format::CodexEncrypted,
                    keychain: None,
                });
            }
            out
        }
    }
}

fn codex_config(home: &Path) -> Option<toml_edit::DocumentMut> {
    fs::read_to_string(home.join("config.toml"))
        .ok()
        .and_then(|text| text.parse().ok())
}

fn valid_entry(format: Format, entry: &Value) -> bool {
    if !matches!(format, Format::Claude)
        && entry
            .get("server_name")
            .and_then(Value::as_str)
            .is_some_and(|name| name.starts_with("executor:") || name.starts_with("ema-idp:"))
    {
        return false;
    }
    let (access, refresh, url) = match format {
        Format::Claude => (
            entry.get("accessToken"),
            entry.get("refreshToken"),
            entry.get("serverUrl"),
        ),
        _ => (
            entry
                .get("access_token")
                .or_else(|| entry.pointer("/token_response/access_token")),
            entry
                .get("refresh_token")
                .or_else(|| entry.pointer("/token_response/refresh_token")),
            entry.get("server_url").or_else(|| entry.get("url")),
        ),
    };
    url.and_then(Value::as_str).is_some_and(|s| !s.is_empty())
        && [access, refresh]
            .into_iter()
            .any(|v| v.and_then(Value::as_str).is_some_and(|s| !s.is_empty()))
}

fn expiry(format: Format, entry: &Value) -> Option<u64> {
    entry
        .get(match format {
            Format::Claude => "expiresAt",
            _ => "expires_at",
        })
        .and_then(Value::as_u64)
}

fn newer(format: Format, candidate: &Value, current: Option<&Value>) -> bool {
    let Some(current) = current.filter(|entry| valid_entry(format, entry)) else {
        return true;
    };
    if candidate == current {
        return false;
    }
    // Store mtimes include provider-login writes, and cannot date an individual token.
    // Equal or unknown expiry does not establish freshness: keep the destination intact.
    match (expiry(format, candidate), expiry(format, current)) {
        (Some(new), Some(old)) => new > old,
        _ => false,
    }
}

fn codex_key(entry: &Value) -> Option<String> {
    let name = entry
        .get("server_name")?
        .as_str()?
        .strip_prefix("local:")
        .unwrap_or(entry.get("server_name")?.as_str()?);
    // Enterprise identities are intentionally tied to a CLI profile, never shared.
    if name.starts_with("ema-idp:") || name.starts_with("executor:") {
        return None;
    }
    let url = entry
        .get("url")
        .or_else(|| entry.get("server_url"))?
        .as_str()?;
    let identity = json!({"type":"http", "url":url, "headers":{}});
    let hash = hash(identity.to_string().as_bytes());
    Some(format!("{name}|{}", &hash[..16]))
}

fn codex_file_entry(entry: &Value) -> Value {
    if entry.get("token_response").is_none() {
        return entry.clone();
    }
    let mut value = entry.clone();
    let map = value.as_object_mut().expect("token object");
    let tokens = map.remove("token_response").unwrap();
    if let Some(url) = map.remove("url") {
        map.insert("server_url".into(), url);
    }
    for key in ["access_token", "refresh_token"] {
        if let Some(token) = tokens.get(key) {
            map.insert(key.into(), token.clone());
        }
    }
    map.insert(
        "scopes".into(),
        tokens
            .get("scope")
            .and_then(Value::as_str)
            .map(|scope| json!(scope.split_whitespace().collect::<Vec<_>>()))
            .unwrap_or(json!([])),
    );
    value
}

fn codex_stored_entry(entry: &Value) -> Value {
    if entry.get("token_response").is_some() {
        return entry.clone();
    }
    let mut value = entry.clone();
    let map = value.as_object_mut().expect("token object");
    if let Some(url) = map.remove("server_url") {
        map.insert("url".into(), url);
    }
    let mut tokens = Map::new();
    for key in ["access_token", "refresh_token"] {
        if let Some(token) = map.remove(key) {
            tokens.insert(key.into(), token);
        }
    }
    tokens.insert("token_type".into(), json!("Bearer"));
    if let Some(scopes) = map
        .remove("scopes")
        .and_then(|scopes| scopes.as_array().cloned())
    {
        tokens.insert(
            "scope".into(),
            json!(
                scopes
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        );
    }
    map.remove("executor_owned");
    map.insert("token_response".into(), Value::Object(tokens));
    value
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn object(bytes: &[u8]) -> io::Result<Value> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| io::Error::other("invalid MCP credential JSON"))?;
    if !value.is_object() {
        return Err(io::Error::other("expected MCP credential object"));
    }
    Ok(value)
}

fn read_file(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            return Err(io::Error::other(
                "MCP credential path is not a regular file",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
        _ => {}
    }
    fs::read(path).map(Some)
}

fn owned_path(home: &Path, path: &Path) -> io::Result<()> {
    let relative = path
        .strip_prefix(home)
        .map_err(|_| io::Error::other("MCP path outside account home"))?;
    let mut at = home.to_owned();
    for part in relative.components() {
        at.push(part);
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::other("MCP account path is a symlink"));
            }
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    Ok(())
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    // Refuse links, including dangling ones; account files must never redirect into defaults.
    let _ = read_file(path)?;
    let temp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = private_options().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests;
