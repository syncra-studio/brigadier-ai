//! A cheap sign that an agent's sign-in has just stored its login, read without running the
//! CLI and without ever reading the credential: only where the CLI keeps it, and when that last
//! changed. A sign-in watches it so the agent is asked whether it is signed in the moment the
//! login lands, seconds before the CLI says so. The CLI's own answer still decides.
//!
//! Where each keeps it (checked 2026-10-20 against Claude Code 2.1.303 and codex-cli 0.163.0):
//! - Claude Code on macOS: a Keychain item, service `Claude Code[-custom-oauth]-credentials`,
//!   with `-<first 8 hex of sha256(NFC config dir)>` appended when the config dir is moved
//!   (`CLAUDE_CONFIG_DIR`, or `CLAUDE_SECURESTORAGE_CONFIG_DIR` ahead of it), account `$USER`.
//!   Only its attributes are read (`mdat`, when it last changed). Its file fallback is watched too.
//! - Claude Code elsewhere: `<config dir>/.credentials.json`.
//! - Codex: `<CODEX_HOME>/auth.json`, unless `config.toml` keeps the login in the system's
//!   keyring instead (`cli_auth_credentials_store`), which isn't watched.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::cli::CliEnv;
use crate::model::ProviderKind;

/// Where an agent keeps its login, watched for a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginMark {
    sources: Vec<Source>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// A Keychain item's attributes (macOS).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Keychain { service: String, account: String },
    /// A file's size and modification time.
    File(PathBuf),
}

impl LoginMark {
    /// Where the agent run with `env` keeps its login; `None` when that can't be watched.
    pub fn of(kind: ProviderKind, env: &CliEnv) -> Option<Self> {
        let set = |name: &str| env.var(name).filter(|value| !value.is_empty());
        match kind {
            ProviderKind::Claude => {
                let config = match set("CLAUDE_CONFIG_DIR") {
                    Some(dir) => PathBuf::from(dir),
                    None => env.home()?.join(".claude"),
                };
                let mut sources = Vec::new();
                if cfg!(target_os = "macos") {
                    sources.push(Source::Keychain {
                        service: claude_keychain_service(env, &config)?,
                        account: claude_keychain_account(env.var("USER")),
                    });
                }
                sources.push(Source::File(config.join(".credentials.json")));
                Some(Self { sources })
            }
            ProviderKind::Codex => {
                let home = match set("CODEX_HOME") {
                    Some(dir) => PathBuf::from(dir),
                    None => env.home()?.join(".codex"),
                };
                let config = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
                codex_stores_in_file(&config).then(|| Self::file(home.join("auth.json")))
            }
        }
    }

    /// Watches one file (a test's stand-in for where an agent keeps its login).
    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self {
            sources: vec![Source::File(path.into())],
        }
    }

    /// Where the login stands now, as an opaque stamp that changes when it is stored; `None`
    /// when it can't be read. Never holds the credential.
    pub async fn read(&self) -> Option<String> {
        let sources = self.sources.clone();
        tokio::task::spawn_blocking(move || {
            let mut stamp = String::new();
            for source in &sources {
                stamp.push_str(&source.read()?);
                stamp.push('\n');
            }
            Some(stamp)
        })
        .await
        .ok()?
    }
}

impl Source {
    fn read(&self) -> Option<String> {
        match self {
            Self::File(path) => file_stamp(path),
            Self::Keychain { service, account } => keychain_stamp(service, account),
        }
    }
}

fn file_stamp(path: &Path) -> Option<String> {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let modified = meta
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            Some(format!("{}:{}", modified.as_nanos(), meta.len()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some("none".into()),
        Err(_) => None,
    }
}

/// The item's creation and modification dates, from its attributes only (no data is asked
/// for, so nothing secret is read and no access prompt shows).
#[cfg(target_os = "macos")]
fn keychain_stamp(service: &str, account: &str) -> Option<String> {
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};
    /// `errSecItemNotFound`.
    const NOT_FOUND: i32 = -25300;

    let found = ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(service)
        .account(account)
        .load_attributes(true)
        .limit(Limit::Max(1))
        .search();
    match found {
        Ok(items) => {
            let attributes = items.first()?.simplify_dict()?;
            let date = |key: &str| attributes.get(key).cloned().unwrap_or_default();
            Some(format!("{} {}", date("cdat"), date("mdat")))
        }
        Err(error) if error.code() == NOT_FOUND => Some("none".into()),
        Err(_) => None,
    }
}

#[cfg(not(target_os = "macos"))]
fn keychain_stamp(_service: &str, _account: &str) -> Option<String> {
    None
}

/// Claude Code's Keychain service for the config dir `config`; `None` when the name can't be
/// worked out here (a path that isn't plain ASCII would need Unicode normalising first).
fn claude_keychain_service(env: &CliEnv, config: &Path) -> Option<String> {
    let oauth = if env
        .var("CLAUDE_CODE_CUSTOM_OAUTH_URL")
        .is_some_and(|url| !url.is_empty())
    {
        "-custom-oauth"
    } else {
        ""
    };
    let hashed = match env.var("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
        Some(dir) if dir.is_empty() => None,
        Some(dir) => Some(dir),
        None => env
            .var("CLAUDE_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map(|_| config.as_os_str()),
    };
    let suffix = match hashed {
        None => String::new(),
        Some(dir) => format!("-{}", short_hash(dir)?),
    };
    Some(format!("Claude Code{oauth}-credentials{suffix}"))
}

/// The first 8 hex digits of the path's SHA-256, as Claude Code names its Keychain item. Only
/// an ASCII path, which Unicode normalisation leaves as it is.
fn short_hash(path: &OsStr) -> Option<String> {
    use sha2::{Digest, Sha256};
    let path = path.to_str().filter(|path| path.is_ascii())?;
    let digest = Sha256::digest(path.as_bytes());
    Some(
        digest[..4]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// Claude Code's Keychain account: `$USER` when it is a plain name, else its fixed fallback.
fn claude_keychain_account(user: Option<&OsStr>) -> String {
    let plain = |name: &&str| {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    user.and_then(OsStr::to_str)
        .filter(plain)
        .map_or_else(|| "claude-code-user".to_owned(), str::to_owned)
}

/// Whether Codex keeps its login in `auth.json`, from its `config.toml` (`file` by default;
/// `keyring` and `auto` use the system's keyring).
fn codex_stores_in_file(config: &str) -> bool {
    let Ok(document) = config.parse::<toml_edit::Document<String>>() else {
        return true;
    };
    document
        .get("cli_auth_credentials_store")
        .and_then(|item| item.as_str())
        .is_none_or(|store| store == "file")
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    fn env(vars: &[(&str, &str)]) -> CliEnv {
        CliEnv::from_vars(
            vars.iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        )
    }

    #[test]
    fn claude_names_its_keychain_item_by_its_config_dir() {
        let config = Path::new("/Users/me/.claude");
        // The default config dir has the bare name.
        assert_eq!(
            claude_keychain_service(&env(&[("HOME", "/Users/me")]), config).as_deref(),
            Some("Claude Code-credentials")
        );
        // A moved one hashes it (sha256("/tmp/x") = 2e56aa36…).
        let moved = env(&[("CLAUDE_CONFIG_DIR", "/tmp/x")]);
        assert_eq!(
            claude_keychain_service(&moved, Path::new("/tmp/x")).as_deref(),
            Some("Claude Code-credentials-2e56aa36")
        );
        let custom = env(&[
            ("CLAUDE_CONFIG_DIR", "/tmp/x"),
            ("CLAUDE_CODE_CUSTOM_OAUTH_URL", "https://example.com"),
        ]);
        assert_eq!(
            claude_keychain_service(&custom, Path::new("/tmp/x")).as_deref(),
            Some("Claude Code-custom-oauth-credentials-2e56aa36")
        );
        // Its secure-storage dir wins; set empty, the bare name.
        let storage = env(&[
            ("CLAUDE_CONFIG_DIR", "/elsewhere"),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/tmp/x"),
        ]);
        assert_eq!(
            claude_keychain_service(&storage, Path::new("/elsewhere")).as_deref(),
            Some("Claude Code-credentials-2e56aa36")
        );
        let bare = env(&[
            ("CLAUDE_CONFIG_DIR", "/elsewhere"),
            ("CLAUDE_SECURESTORAGE_CONFIG_DIR", ""),
        ]);
        assert_eq!(
            claude_keychain_service(&bare, Path::new("/elsewhere")).as_deref(),
            Some("Claude Code-credentials")
        );
        // A path that would need normalising isn't guessed at.
        let accented = env(&[("CLAUDE_CONFIG_DIR", "/Users/zoé/.claude")]);
        assert_eq!(
            claude_keychain_service(&accented, Path::new("/Users/zoé/.claude")),
            None
        );

        assert_eq!(claude_keychain_account(Some(OsStr::new("me"))), "me");
        assert_eq!(
            claude_keychain_account(Some(OsStr::new("me two"))),
            "claude-code-user"
        );
        assert_eq!(claude_keychain_account(None), "claude-code-user");
    }

    #[test]
    fn codex_is_watched_only_when_its_login_is_a_file() {
        assert!(codex_stores_in_file(""));
        assert!(codex_stores_in_file("model = \"gpt-5\"\n"));
        assert!(codex_stores_in_file(
            "cli_auth_credentials_store = \"file\"\n"
        ));
        assert!(!codex_stores_in_file(
            "cli_auth_credentials_store = \"keyring\"\n"
        ));
        assert!(!codex_stores_in_file(
            "cli_auth_credentials_store = \"auto\"\n"
        ));
        // One it can't read is the default.
        assert!(codex_stores_in_file("not toml ["));

        let dir =
            std::env::temp_dir().join(format!("brigadier-login-mark-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let codex = env(&[("CODEX_HOME", dir.to_str().unwrap())]);
        assert_eq!(
            LoginMark::of(ProviderKind::Codex, &codex),
            Some(LoginMark::file(dir.join("auth.json")))
        );
        std::fs::write(
            dir.join("config.toml"),
            "cli_auth_credentials_store = \"keyring\"\n",
        )
        .unwrap();
        assert_eq!(LoginMark::of(ProviderKind::Codex, &codex), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn a_stored_login_changes_the_mark() {
        let dir =
            std::env::temp_dir().join(format!("brigadier-login-mark-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mark = LoginMark::file(dir.join("auth.json"));
        let before = mark.read().await.unwrap();
        assert_eq!(mark.read().await.unwrap(), before);
        std::fs::write(dir.join("auth.json"), "{}").unwrap();
        let stored = mark.read().await.unwrap();
        assert_ne!(stored, before);
        std::fs::write(dir.join("auth.json"), "{\"again\":1}").unwrap();
        assert_ne!(mark.read().await.unwrap(), stored);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Claude Code's own item for a scratch config dir that has none: read as absent, not as
    /// unreadable, without any prompt.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_missing_keychain_item_reads_as_none() {
        let dir = format!("/tmp/brigadier-login-mark-{}", uuid::Uuid::new_v4());
        let mark = LoginMark::of(
            ProviderKind::Claude,
            &env(&[("CLAUDE_CONFIG_DIR", &dir), ("USER", "nobody-here")]),
        )
        .unwrap();
        assert_eq!(mark.read().await.as_deref(), Some("none\nnone\n"));
    }

    /// A scratch Keychain item for a scratch config dir appearing, then changing, changes the
    /// mark. Run by hand (`--ignored`): it writes to the login keychain, then removes the item.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    #[ignore = "writes a scratch item to the login keychain"]
    async fn a_keychain_item_appearing_or_changing_changes_the_mark() {
        let dir = format!("/tmp/brigadier-login-mark-{}", uuid::Uuid::new_v4());
        let user = std::env::var("USER").unwrap();
        let claude = env(&[("CLAUDE_CONFIG_DIR", &dir), ("USER", &user)]);
        let mark = LoginMark::of(ProviderKind::Claude, &claude).unwrap();
        let service = claude_keychain_service(&claude, Path::new(&dir)).unwrap();
        let security = |args: &[&str]| {
            std::process::Command::new("/usr/bin/security")
                .args(args)
                .output()
                .unwrap()
        };
        let before = mark.read().await.unwrap();
        security(&[
            "add-generic-password",
            "-s",
            &service,
            "-a",
            &user,
            "-w",
            "scratch",
        ]);
        let stored = mark.read().await.unwrap();
        // Its dates change by the second.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        security(&[
            "add-generic-password",
            "-U",
            "-s",
            &service,
            "-a",
            &user,
            "-w",
            "again",
        ]);
        let again = mark.read().await.unwrap();
        security(&["delete-generic-password", "-s", &service, "-a", &user]);
        assert_ne!(stored, before);
        assert_ne!(again, stored);
        assert!(!stored.contains("scratch") && !again.contains("again"));
        assert_eq!(mark.read().await.unwrap(), before);
    }
}
