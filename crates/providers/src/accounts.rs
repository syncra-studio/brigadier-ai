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
//!
//! MCP servers are resolved from the main catalog and injected at launch; no servers are
//! copied into an account's credential-bearing config file.

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
            // The installed plugins, and the settings that enable them and name their
            // marketplaces (`enabledPlugins`, `extraKnownMarketplaces`): the plugins folder
            // alone loads none of them. Claude writes the settings through the link.
            Optional("plugins"),
            Optional("settings.json"),
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
            // The installed plugins (`config.toml` enables them) and the user's hooks.
            Optional("plugins"),
            Optional("hooks.json"),
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
                key.to_str().is_none_or(|key| {
                    !auth_vars(kind).contains(&key)
                        && key != home_var(kind)
                        && !(kind == ProviderKind::Claude
                            && key == "CLAUDE_SECURESTORAGE_CONFIG_DIR")
                })
            })
            .collect();
        vars.push((home_var(kind).into(), home.as_os_str().to_owned()));
        self.with_vars(vars)
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

/// Claude's config file for the user's own login, where their user servers (`mcpServers`) and
/// each project's local ones (`projects.<path>.mcpServers`) are: `.claude.json` in
/// `CLAUDE_CONFIG_DIR`, else in the home folder, as Claude Code itself picks it (an old
/// `.config.json` in the config folder wins when present).
pub fn main_config_file(env: &CliEnv) -> Option<PathBuf> {
    let set = env
        .var(home_var(ProviderKind::Claude))
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let home = env.home().or_else(|| set.clone())?;
    Some(brigadier_registry::claude_config::config_file(
        &home,
        set.as_deref(),
    ))
}

/// Deletes an account's home. Links are removed, never followed: what they point to in the
/// main home stays.
pub fn remove_home(home: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(home) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
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

/// Link a Brigadier-owned path without changing its source.
#[cfg(unix)]
pub fn link_to(target: &Path, link: &Path, _dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows: a folder is linked by a junction (no privilege needed), a file by a hard link (a
/// CLI that replaces the file then keeps its own copy).
#[cfg(windows)]
pub fn link_to(target: &Path, link: &Path, dir: bool) -> io::Result<()> {
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

    /// A fresh folder, removed after the test however it ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-accounts-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
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
    }

    #[cfg(unix)]
    #[test]
    fn claudes_main_config_file_follows_its_config_dir_or_the_home() {
        let set = CliEnv::from_vars([
            (OsString::from("HOME"), OsString::from("/home/me")),
            (OsString::from("CLAUDE_CONFIG_DIR"), OsString::from("/cfg")),
        ]);
        assert_eq!(
            main_config_file(&set),
            Some(PathBuf::from("/cfg/.claude.json"))
        );
        let unset = CliEnv::from_vars([(OsString::from("HOME"), OsString::from("/home/me"))]);
        assert_eq!(
            main_config_file(&unset),
            Some(PathBuf::from("/home/me/.claude.json"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_account_shares_the_plugins_and_the_settings_that_enable_them() {
        let root = scratch("plugins");
        for (kind, names) in [
            (ProviderKind::Claude, &["plugins", "settings.json"][..]),
            (
                ProviderKind::Codex,
                &["plugins", "hooks.json", "config.toml"][..],
            ),
        ] {
            let main = root.join(format!("{kind}-main"));
            std::fs::create_dir_all(main.join("plugins")).unwrap();
            for file in ["settings.json", "hooks.json", "config.toml"] {
                std::fs::write(main.join(file), "").unwrap();
            }
            let home = root.join(format!("{kind}-account"));
            prepare_home(kind, &main, &home).unwrap();
            for name in names {
                assert_eq!(
                    std::fs::read_link(home.join(name)).unwrap(),
                    main.join(name)
                );
            }
        }
    }

    /// Real CLIs on throwaway homes: run with `--ignored`.
    #[cfg(unix)]
    mod live {
        use super::*;
        use std::process::Command;

        fn root(name: &str) -> PathBuf {
            let dir = PathBuf::from(format!(
                "/tmp/brigadier-accounts-live-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("os-home")).unwrap();
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            std::fs::create_dir_all(dir.join("project")).unwrap();
            // A stdio MCP server that answers `initialize` and `tools/list`.
            std::fs::write(
                dir.join("bin/server.py"),
                "import json, sys\nfor line in sys.stdin:\n    req = json.loads(line)\n    if 'id' not in req:\n        continue\n    m = req.get('method')\n    r = {}\n    if m == 'initialize':\n        r = {'protocolVersion': req['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fixture', 'version': '0.1.0'}}\n    elif m == 'tools/list':\n        r = {'tools': [{'name': 'probe', 'description': 'fixture', 'inputSchema': {'type': 'object', 'properties': {}}}]}\n    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': r}), flush=True)\n",
            )
            .unwrap();
            let market = dir.join("market");
            std::fs::create_dir_all(market.join(".claude-plugin")).unwrap();
            std::fs::create_dir_all(market.join("alpha/.claude-plugin")).unwrap();
            std::fs::write(
                market.join(".claude-plugin/marketplace.json"),
                r#"{"name":"w05c-local","owner":{"name":"w05c"},"plugins":[{"name":"alpha","source":"./alpha","description":"fixture"}]}"#,
            )
            .unwrap();
            std::fs::write(
                market.join("alpha/.claude-plugin/plugin.json"),
                r#"{"name":"alpha","version":"0.1.0","description":"fixture"}"#,
            )
            .unwrap();
            std::fs::write(
                market.join("alpha/.mcp.json"),
                format!(
                    r#"{{"mcpServers":{{"shared":{{"command":"/usr/bin/python3","args":["{}","alpha"]}}}}}}"#,
                    dir.join("bin/server.py").display()
                ),
            )
            .unwrap();
            dir
        }

        fn cli(dir: &Path, program: &str, var: &str, home: &Path, args: &[&str]) -> String {
            let out = Command::new(program)
                .args(args)
                .current_dir(dir.join("project"))
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("HOME", dir.join("os-home"))
                .env(var, home)
                .env("TERM", "dumb")
                .env("NO_COLOR", "1")
                .env("DISABLE_AUTOUPDATER", "1")
                .output()
                .unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            println!(
                "$ {var}={} {program} {}\n{text}",
                home.display(),
                args.join(" ")
            );
            assert!(out.status.success(), "{program} {args:?} failed");
            text
        }

        #[test]
        #[ignore = "runs the real codex on throwaway homes"]
        fn live_a_codex_mcp_add_on_an_account_survives_the_next_launch() {
            let dir = root("codex");
            let main = dir.join("main");
            std::fs::create_dir_all(&main).unwrap();
            let codex = |home: &Path, args: &[&str]| cli(&dir, "codex", "CODEX_HOME", home, args);
            codex(
                &main,
                &["mcp", "add", "user-one", "--", "/usr/bin/python3", "x.py"],
            );
            codex(
                &main,
                &[
                    "plugin",
                    "marketplace",
                    "add",
                    dir.join("market").to_str().unwrap(),
                ],
            );
            codex(&main, &["plugin", "add", "alpha@w05c-local"]);
            let main_text = std::fs::read_to_string(main.join("config.toml")).unwrap();

            let extra = dir.join("extra");
            prepare_home(ProviderKind::Codex, &main, &extra).unwrap();
            let listed = codex(&extra, &["mcp", "list"]);
            assert!(
                listed.contains("user-one") && listed.contains("shared"),
                "{listed}"
            );
            // Added inside a session on the account, as `codex mcp add` does there.
            codex(
                &extra,
                &["mcp", "add", "in-session", "--", "/usr/bin/python3", "y.py"],
            );
            // The next launch prepares the home again.
            prepare_home(ProviderKind::Codex, &main, &extra).unwrap();
            for home in [&extra, &main] {
                let listed = codex(home, &["mcp", "list", "--json"]);
                assert!(listed.contains("\"in-session\""), "{listed}");
            }
            // It went in through Codex's own writer; the rest of the user's file is as it was.
            let now = std::fs::read_to_string(main.join("config.toml")).unwrap();
            let added =
                "[mcp_servers.in-session]\ncommand = \"/usr/bin/python3\"\nargs = [\"y.py\"]\n\n";
            assert_eq!(now.replace(added, ""), main_text, "{now}");
            assert!(
                std::fs::symlink_metadata(extra.join("config.toml"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}
