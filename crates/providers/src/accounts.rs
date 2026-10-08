//! More than one login per CLI: each extra account is a CLI home of its own that only Brigadier
//! uses (`CLAUDE_CONFIG_DIR` for Claude, `CODEX_HOME` for Codex). The CLI keeps its own login
//! in it, in its own way (Claude Code keys its Keychain entry on the folder; Codex writes
//! `auth.json` or a keyring entry keyed on it); Brigadier never reads or writes a credential.
//! The user's own login, the one their terminal uses, stays the first account and is never
//! touched.
//!
//! Every account shares the user's chat history and their personal CLI files: the account's
//! home links each of [`shared`] back to the main home. Both CLIs resume a session only from
//! their own home (Claude from `projects`, Codex from `sessions`), so the links are what let a
//! conversation move from one account to another and still resume (checked with Claude Code
//! 2.1.295 and codex-cli 0.161.0, `docs/evidence/2026-10-09-accounts.md`).
//!
//! A login in the environment would win over the home's own login, so an account's processes
//! run without the variables in [`auth_vars`].

use std::io;
use std::path::{Path, PathBuf};

use crate::cli::CliEnv;
use crate::model::ProviderKind;

/// The variable that moves a CLI's home.
pub fn home_var(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Claude => "CLAUDE_CONFIG_DIR",
        ProviderKind::Codex => "CODEX_HOME",
    }
}

/// Variables that sign a CLI in on their own, ahead of the login kept in its home: Claude
/// Code's API key, bearer token and long-lived OAuth token
/// (<https://code.claude.com/docs/en/authentication#authentication-precedence>); Codex's
/// API keys and access token (named in codex-cli 0.161.0's binary).
pub fn auth_vars(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        ProviderKind::Claude => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ],
        ProviderKind::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"],
    }
}

/// What an account's home links back to the main home: the session history the CLI resumes
/// from, and the user's personal instructions, agents, skills and the like. Folders
/// ([`Shared::Dir`]) are made in the main home when missing, so a session written from any
/// account lands where every other account (and the user's terminal) finds it.
pub fn shared(kind: ProviderKind) -> &'static [Shared] {
    use Shared::{Dir, Optional};
    match kind {
        ProviderKind::Claude => &[
            Dir("projects"),
            Optional("CLAUDE.md"),
            Optional("agents"),
            Optional("commands"),
            Optional("skills"),
            Optional("plugins"),
        ],
        ProviderKind::Codex => &[
            Dir("sessions"),
            Dir("archived_sessions"),
            Dir("generated_images"),
            Optional("config.toml"),
            Optional("AGENTS.md"),
            Optional("skills"),
            Optional("prompts"),
            Optional("rules"),
        ],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shared {
    /// A folder every account needs: made in the main home when missing.
    Dir(&'static str),
    /// Linked only while the main home has it.
    Optional(&'static str),
}

impl Shared {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dir(name) | Self::Optional(name) => name,
        }
    }
}

/// The user's own home for `kind`, the one their terminal uses: what `env` sets, else the
/// default (`~/.claude`, `~/.codex`).
pub fn main_home(kind: ProviderKind, env: &CliEnv) -> Option<PathBuf> {
    match env.var(home_var(kind)).filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => env.home().map(|home| {
            home.join(match kind {
                ProviderKind::Claude => ".claude",
                ProviderKind::Codex => ".codex",
            })
        }),
    }
}

impl CliEnv {
    /// This environment for an account whose CLI home is `home`: the home set, and no login
    /// variable left that would win over the home's own login.
    pub fn for_account(&self, kind: ProviderKind, home: &Path) -> CliEnv {
        let mut vars: Vec<_> = self
            .vars()
            .into_iter()
            .filter(|(key, _)| {
                key.to_str()
                    .is_none_or(|key| !auth_vars(kind).contains(&key) && key != home_var(kind))
            })
            .collect();
        vars.push((home_var(kind).into(), home.as_os_str().to_owned()));
        CliEnv::from_vars(vars)
    }
}

/// Makes an account's home (owner-only) and links it to the main home `main` (see
/// [`shared`]). Safe to repeat: an existing link is kept, and so is anything the CLI put in
/// its place. A link the main home no longer backs is left for the CLI to report.
///
/// A new Claude home starts with a `.claude.json` that says its first-run setup is done
/// (`hasCompletedOnboarding`, the key Claude Code 2.1.295 keeps there): the user went
/// through it with their own login, and a terminal that resumes a session on this account
/// must not ask again. Nothing else of the user's own `.claude.json` is copied.
pub fn prepare_home(kind: ProviderKind, main: &Path, home: &Path) -> io::Result<()> {
    create_private_dir(home)?;
    if kind == ProviderKind::Claude {
        let config = home.join(".claude.json");
        if std::fs::symlink_metadata(&config).is_err() {
            std::fs::write(&config, "{\n  \"hasCompletedOnboarding\": true\n}\n")?;
        }
    }
    for entry in shared(kind) {
        let target = main.join(entry.name());
        if let Shared::Dir(_) = entry {
            std::fs::create_dir_all(&target)?;
        }
        let link = home.join(entry.name());
        if std::fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&target) else {
            continue;
        };
        link_to(&target, &link, meta.is_dir())?;
    }
    Ok(())
}

/// Deletes an account's home. Links are removed, never followed: what they point to in the
/// main home stays.
pub fn remove_home(home: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(home) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(unix)]
fn link_to(target: &Path, link: &Path, _dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows: a folder is linked by a junction (no privilege needed), a file by a hard link (a
/// CLI that replaces the file then keeps its own copy).
#[cfg(windows)]
fn link_to(target: &Path, link: &Path, dir: bool) -> io::Result<()> {
    if dir {
        junction::create(target, link)
    } else {
        std::fs::hard_link(target, link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-accounts-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_account_runs_on_its_own_home_with_no_login_variable() {
        let env = CliEnv::from_vars([
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from("HOME"), OsString::from("/home/me")),
            (
                OsString::from("ANTHROPIC_API_KEY"),
                OsString::from("sentinel-key"),
            ),
            (
                OsString::from("ANTHROPIC_AUTH_TOKEN"),
                OsString::from("sentinel-token"),
            ),
            (
                OsString::from("CLAUDE_CODE_OAUTH_TOKEN"),
                OsString::from("sentinel-oauth"),
            ),
            (
                OsString::from("OPENAI_API_KEY"),
                OsString::from("sentinel-openai"),
            ),
            (
                OsString::from("CODEX_API_KEY"),
                OsString::from("sentinel-codex"),
            ),
            (
                OsString::from("CODEX_ACCESS_TOKEN"),
                OsString::from("sentinel-access"),
            ),
            (
                OsString::from("CLAUDE_CONFIG_DIR"),
                OsString::from("/elsewhere"),
            ),
        ]);
        let claude = env.for_account(ProviderKind::Claude, Path::new("/data/accounts/a1"));
        let codex = env.for_account(ProviderKind::Codex, Path::new("/data/accounts/a2"));
        for (account, kind) in [
            (&claude, ProviderKind::Claude),
            (&codex, ProviderKind::Codex),
        ] {
            for name in auth_vars(kind) {
                assert_eq!(account.var(name), None, "{name} reached a {kind} account");
            }
            let leaked = account
                .vars()
                .into_iter()
                .filter(|(key, value)| {
                    value.to_string_lossy().starts_with("sentinel")
                        && auth_vars(kind).contains(&key.to_str().unwrap())
                })
                .count();
            assert_eq!(leaked, 0);
            assert_eq!(account.var("PATH"), Some(std::ffi::OsStr::new("/usr/bin")));
        }
        assert_eq!(
            claude.var("CLAUDE_CONFIG_DIR"),
            Some(std::ffi::OsStr::new("/data/accounts/a1"))
        );
        assert_eq!(
            codex.var("CODEX_HOME"),
            Some(std::ffi::OsStr::new("/data/accounts/a2"))
        );
        // The user's own login (the base environment) keeps every variable.
        assert_eq!(
            env.var("ANTHROPIC_API_KEY"),
            Some(std::ffi::OsStr::new("sentinel-key"))
        );
    }

    #[test]
    fn the_main_home_is_the_variable_or_the_default() {
        let set = CliEnv::from_vars([(OsString::from("CODEX_HOME"), OsString::from("/x"))]);
        assert_eq!(
            main_home(ProviderKind::Codex, &set),
            Some(PathBuf::from("/x"))
        );
        let unset = CliEnv::from_vars([(OsString::from("HOME"), OsString::from("/home/me"))]);
        assert_eq!(
            main_home(ProviderKind::Claude, &unset),
            Some(PathBuf::from("/home/me/.claude"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_home_links_the_shared_history_and_removing_it_keeps_the_main_home() {
        let root = scratch("links");
        let main = root.join("main");
        std::fs::create_dir_all(main.join("skills")).unwrap();
        std::fs::write(main.join("CLAUDE.md"), "be brief").unwrap();
        let home = root.join("account");
        prepare_home(ProviderKind::Claude, &main, &home).unwrap();
        // `projects` is made in the main home, and linked.
        assert!(main.join("projects").is_dir());
        for name in ["projects", "skills", "CLAUDE.md"] {
            assert_eq!(
                std::fs::read_link(home.join(name)).unwrap(),
                main.join(name),
                "{name}"
            );
        }
        // Missing in the main home: not linked.
        assert!(std::fs::symlink_metadata(home.join("agents")).is_err());
        // Its own config, with the first-run setup done; kept as the CLI leaves it.
        let config = home.join(".claude.json");
        let first: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(first["hasCompletedOnboarding"], true);
        std::fs::write(&config, "{}").unwrap();
        prepare_home(ProviderKind::Claude, &main, &home).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "{}");
        // Owner-only.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&home).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // A session written through the account lands in the main home.
        std::fs::create_dir_all(home.join("projects/-work")).unwrap();
        std::fs::write(home.join("projects/-work/s1.jsonl"), "{}").unwrap();
        assert!(main.join("projects/-work/s1.jsonl").is_file());
        // Again: nothing changes; something the CLI put in a link's place stays.
        std::fs::create_dir_all(main.join("agents")).unwrap();
        std::fs::create_dir(home.join("agents")).unwrap();
        prepare_home(ProviderKind::Claude, &main, &home).unwrap();
        assert!(
            !std::fs::symlink_metadata(home.join("agents"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::write(home.join(".claude.json"), "{}").unwrap();

        remove_home(&home).unwrap();
        assert!(!home.exists());
        assert!(main.join("projects/-work/s1.jsonl").is_file());
        assert_eq!(
            std::fs::read_to_string(main.join("CLAUDE.md")).unwrap(),
            "be brief"
        );
        assert!(main.join("skills").is_dir());
        // Already gone: fine.
        remove_home(&home).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_home_shares_sessions_archive_and_config() {
        let root = scratch("codex");
        let main = root.join("main");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(main.join("config.toml"), "model = \"x\"\n").unwrap();
        let home = root.join("account");
        prepare_home(ProviderKind::Codex, &main, &home).unwrap();
        for name in ["sessions", "archived_sessions"] {
            assert!(main.join(name).is_dir(), "{name}");
            assert_eq!(
                std::fs::read_link(home.join(name)).unwrap(),
                main.join(name)
            );
        }
        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            "model = \"x\"\n"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
