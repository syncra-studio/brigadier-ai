//! The `CODEX_HOME` Codex threads run from (issue #121), so their output hook can run: Codex
//! loads hooks only from its home's `hooks.json` and trusts one only through a
//! `hooks.state."<key>"` `trusted_hash` in that home's own `config.toml` (`-c` doesn't count,
//! and `app-server` has no bypass; docs/research/long-sessions/PROBES.md P6). So the threads of
//! each account run from a home of Brigadier's, under its data folder, brought up to date at
//! every launch and kept: Codex records a thread's rollout by its path through this home (in
//! the account's databases), so the home must outlive the session, or the thread couldn't be
//! unarchived and resumed again.
//!
//! - Every entry of the account's home (`base`: the user's own, or an extra account's) is
//!   linked back, the way an extra account's home links the user's: the login (`auth.json`;
//!   a token refresh writes through the link, checked with a fake token server on
//!   codex-cli 0.163.0), the session history resume reads (`sessions/`), installed plugins,
//!   skills and the rest. Its databases stay where they are: the CLI runs with
//!   `CODEX_SQLITE_HOME` on `base` ([`SQLITE_HOME`]), which moves every one of them.
//! - `config.toml` is a copy made at each launch: Codex writes the trust entries into it, and
//!   what it writes back there stays out of the user's file.
//! - `hooks.json` holds the user's own hooks (`base`'s, which the session ran with before) and
//!   Brigadier's `PostToolUse` hook after them. The user's keep the trust they had, under their
//!   new keys ([`ThreadHome::carried_trust`]); Brigadier's is trusted with the hash Codex
//!   gives it (`hooks/list`), before the thread starts.
//!
//! Only where the login lives in `auth.json` ([`runs_thread_hook`]): a keyring login is keyed
//! on the home's path, which a home of its own wouldn't find.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::cli::CliEnv;
use crate::model::{OutputHook, ProviderKind};

/// The variable that moves every Codex database (state, thread history, goals, logs).
pub(crate) const SQLITE_HOME: &str = "CODEX_SQLITE_HOME";

/// The tools whose calls run Brigadier's hook: the shell and patches, how Codex reads and
/// changes files.
const HOOK_MATCHER: &str = "Bash|apply_patch";

/// What a session home keeps of its own instead of linking: its config and hooks, the
/// databases (which stay in `base` through [`SQLITE_HOME`]), and per-process scratch: logs,
/// temp files, shell snapshots, the Codex app's own control sockets, backups.
fn own_entry(name: &str) -> bool {
    matches!(
        name,
        "config.toml"
            | "hooks.json"
            | "sqlite"
            | "log"
            | "tmp"
            | ".tmp"
            | "shell_snapshots"
            | "app-server-control"
            | "app-server-daemon"
            | "ipc"
    ) || [".sqlite", ".sqlite-shm", ".sqlite-wal", ".lock", ".bak"]
        .iter()
        .any(|end| name.ends_with(end))
}

/// Whether a Codex thread run with `env` can have a home of its own, and so its output hook:
/// its login is kept in `auth.json`, not the system's keyring (`cli_auth_credentials_store`),
/// and hooks are on (`features.hooks`), in the user's `config.toml`, which every account's home
/// shares. Not on Windows, where the login would be a hard link that Codex's token refresh,
/// replacing the file, would part from the account's own.
pub fn runs_thread_hook(env: &CliEnv) -> bool {
    let Some(home) = crate::accounts::main_home(ProviderKind::Codex, env) else {
        return false;
    };
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap_or_default();
    cfg!(unix) && crate::login_mark::codex_stores_in_file(&config) && hooks_on(&config)
}

/// The `CODEX_HOME` an account's threads run from.
#[derive(Debug)]
pub(crate) struct ThreadHome {
    /// The home, as Codex names its hooks' paths (resolved).
    pub dir: PathBuf,
    /// The account's home it links back to (resolved).
    pub base: PathBuf,
    /// The `PostToolUse` group that is Brigadier's hook, after the user's.
    own_group: usize,
}

impl ThreadHome {
    /// The home for `base`'s threads under `root` (one per account home), made or brought up
    /// to date (owner-only): linked back to `base`, a fresh copy of its config, and `hook`
    /// after the user's own hooks. Threads launching side by side may do this at once: every
    /// write replaces a file whole.
    pub(crate) fn prepare(base: &Path, root: &Path, hook: &OutputHook) -> io::Result<Self> {
        // The folders every account's sessions land in exist in `base`, so they are linked.
        for entry in crate::accounts::shared(ProviderKind::Codex) {
            if let crate::accounts::Shared::Dir(name) = entry {
                std::fs::create_dir_all(base.join(name))?;
            }
        }
        let base = base.canonicalize()?;
        let dir = root.join(home_name(&base));
        crate::accounts::create_private_dir(&dir)?;
        let dir = dir.canonicalize()?;
        for entry in std::fs::read_dir(&base)? {
            let entry = entry?;
            let name = entry.file_name();
            let link = dir.join(&name);
            if name.to_str().is_none_or(own_entry) || std::fs::symlink_metadata(&link).is_ok() {
                continue;
            }
            let target = base.join(&name);
            let is_dir = std::fs::metadata(&target).is_ok_and(|meta| meta.is_dir());
            match crate::accounts::link_to(&target, &link, is_dir) {
                Err(err) if err.kind() != io::ErrorKind::AlreadyExists => return Err(err),
                _ => {}
            }
        }
        let config = std::fs::read_to_string(base.join("config.toml")).unwrap_or_default();
        let config = with_base_paths(&config, &base);
        // Switched off since the thread's instructions were written: told to read them itself.
        if !hooks_on(&config) {
            return Err(io::Error::other("the user switched Codex's hooks off"));
        }
        let user_hooks = std::fs::read_to_string(base.join("hooks.json")).ok();
        let (hooks, own_group) = session_hooks(user_hooks.as_deref(), hook);
        let home = Self {
            dir,
            base,
            own_group,
        };
        let own_config = home.dir.join("config.toml");
        let kept = std::fs::read_to_string(&own_config).unwrap_or_default();
        replace_private(
            &own_config,
            &with_kept_trust(&config, &kept, &home.own_key()),
        )?;
        let hooks_file = home.dir.join("hooks.json");
        if std::fs::read_to_string(&hooks_file).ok().as_deref() != Some(hooks.as_str()) {
            replace_private(&hooks_file, &hooks)?;
        }
        Ok(home)
    }

    /// Brigadier's hook's key, as `hooks/list` names it.
    pub(crate) fn own_key(&self) -> String {
        format!(
            "{}:post_tool_use:{}:0",
            self.dir.join("hooks.json").display(),
            self.own_group
        )
    }

    /// The trust the user gave their own hooks (`base`'s `hooks.state`), under the keys the
    /// same hooks have in this home: a hook keeps the trust it had, and one the user never
    /// trusted (or changed since) stays untrusted, Codex comparing the hashes.
    pub(crate) fn carried_trust(&self) -> Map<String, Value> {
        let config = std::fs::read_to_string(self.dir.join("config.toml")).unwrap_or_default();
        carried_trust(&config, &self.base, &self.dir)
    }
}

/// The home's folder name for the account home `base`: the same for every launch on it.
fn home_name(base: &Path) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(base.as_os_str().as_encoded_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `user`'s hooks file (`None`: none) with `hook` as a `PostToolUse` group after the user's,
/// and that group's index. A file that doesn't read as hooks is left out: Codex wouldn't
/// run it either.
fn session_hooks(user: Option<&str>, hook: &OutputHook) -> (String, usize) {
    let mut file = user
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .filter(|file| file.get("hooks").is_some_and(Value::is_object))
        .unwrap_or_else(|| json!({ "hooks": {} }));
    let command = std::iter::once(hook.command.display().to_string())
        .chain(hook.args.iter().cloned())
        .map(|part| crate::claude::shell_quote(&part))
        .collect::<Vec<_>>()
        .join(" ");
    let group = json!({
        "matcher": HOOK_MATCHER,
        "hooks": [{ "type": "command", "command": command, "timeout": hook.timeout_secs }],
    });
    let hooks = file["hooks"].as_object_mut().expect("an object");
    let post = hooks
        .entry("PostToolUse")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !post.is_array() {
        *post = Value::Array(Vec::new());
    }
    let groups = post.as_array_mut().expect("an array");
    groups.push(group);
    let index = groups.len() - 1;
    let text = serde_json::to_string_pretty(&file).unwrap_or_default();
    (text, index)
}

/// Whether the user's `config` leaves Codex's hooks on (`features.hooks`, on by default).
fn hooks_on(config: &str) -> bool {
    let Ok(document) = config.parse::<toml_edit::Document<String>>() else {
        return true;
    };
    document
        .get("features")
        .and_then(|features| features.get("hooks"))
        .and_then(toml_edit::Item::as_bool)
        .is_none_or(|on| on)
}

/// The user's `config` with the trust the home's previous copy (`kept`) gave Brigadier's hook
/// (`own`), so a thread launching meanwhile on the same home never finds it untrusted while
/// the copy is replaced. Codex compares the hashes: a hook changed since stays untrusted until
/// it is trusted again. The user's hooks only ever have the trust `config` gives them now.
/// Just `config` when the two don't make one valid file.
fn with_kept_trust(config: &str, kept: &str, own: &str) -> String {
    let Ok(document) = kept.parse::<toml_edit::Document<String>>() else {
        return config.to_owned();
    };
    let Some(hash) = document
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(|state| state.get(own))
        .and_then(|entry| entry.get("trusted_hash"))
        .and_then(toml_edit::Item::as_str)
    else {
        return config.to_owned();
    };
    let mut text = config.to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    // A JSON string is a valid TOML basic string.
    text.push_str(&format!(
        "\n[hooks.state.{}]\ntrusted_hash = {}\n",
        Value::from(own),
        Value::from(hash)
    ));
    if text.parse::<toml_edit::Document<String>>().is_ok() {
        text
    } else {
        config.to_owned()
    }
}

/// The user's `config` for a copy in another home: a relative `sqlite_home` (Codex takes it
/// from the config file's folder, and over [`SQLITE_HOME`]) named from `base`, where the
/// databases are. Anything else, and a `config` that doesn't parse, as it is.
fn with_base_paths(config: &str, base: &Path) -> String {
    let Ok(document) = config.parse::<toml_edit::Document<String>>() else {
        return config.to_owned();
    };
    let Some(value) = document
        .get("sqlite_home")
        .and_then(toml_edit::Item::as_value)
    else {
        return config.to_owned();
    };
    let (Some(dir), Some(span)) = (value.as_str(), value.span()) else {
        return config.to_owned();
    };
    if dir.is_empty() || dir.starts_with('~') || Path::new(dir).is_absolute() {
        return config.to_owned();
    }
    // A JSON string is a valid TOML basic string.
    let named = Value::from(base.join(dir).display().to_string()).to_string();
    format!("{}{named}{}", &config[..span.start], &config[span.end..])
}

/// `config`'s `hooks.state` entries for hooks of `base` (its `hooks.json` and `config.toml`),
/// keyed on `home` instead, as `hooks.state` table entries.
fn carried_trust(config: &str, base: &Path, home: &Path) -> Map<String, Value> {
    let mut carried = Map::new();
    let Ok(document) = config.parse::<toml_edit::Document<String>>() else {
        return carried;
    };
    let Some(state) = document
        .get("hooks")
        .and_then(|hooks| hooks.get("state"))
        .and_then(toml_edit::Item::as_table_like)
    else {
        return carried;
    };
    for file in ["hooks.json", "config.toml"] {
        let from = format!("{}:", base.join(file).display());
        let to = format!("{}:", home.join(file).display());
        for (key, entry) in state.iter() {
            let Some(rest) = key.strip_prefix(&from) else {
                continue;
            };
            let Some(entry) = entry.as_table_like() else {
                continue;
            };
            let mut fields = Map::new();
            if let Some(hash) = entry.get("trusted_hash").and_then(toml_edit::Item::as_str) {
                fields.insert("trusted_hash".into(), hash.into());
            }
            if let Some(enabled) = entry.get("enabled").and_then(toml_edit::Item::as_bool) {
                fields.insert("enabled".into(), enabled.into());
            }
            if !fields.is_empty() {
                carried.insert(format!("{to}{rest}"), Value::Object(fields));
            }
        }
    }
    carried
}

/// Replaces `path` whole with `text`, in a file only its owner can read: written beside it,
/// then renamed over it, so a CLI reading it meanwhile reads one or the other.
fn replace_private(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.{}", uuid::Uuid::new_v4().simple()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options
        .open(&temp)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder, removed after the test however it ends.
    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch() -> Temp {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-codex-home-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir.canonicalize().unwrap())
    }

    fn hook() -> OutputHook {
        OutputHook {
            command: PathBuf::from("/Applications/Brigadier App.app/brigadierd"),
            args: vec!["hook".into(), "post-tool-use".into(), "--codex".into()],
            env: vec![("BRIGADIER_HOOK_GRANT".into(), "brg_x".into())],
            timeout_secs: 10,
        }
    }

    /// The home links the account's login, history and the rest back, keeps its own config
    /// copy and hooks, and leaves the databases and scratch out; the next launch finds the
    /// same home, with the user's config as it is now.
    #[cfg(unix)]
    #[test]
    fn a_thread_home_links_the_accounts_home_and_keeps_its_own_hooks() {
        let root = scratch();
        let base = root.0.join("base");
        std::fs::create_dir_all(base.join("plugins")).unwrap();
        std::fs::write(base.join("auth.json"), "{}").unwrap();
        std::fs::write(base.join("installation_id"), "id").unwrap();
        std::fs::write(base.join("config.toml"), "model = \"m\"\n").unwrap();
        std::fs::write(base.join("state_5.sqlite"), "").unwrap();
        std::fs::write(base.join("state_5.sqlite-wal"), "").unwrap();
        std::fs::create_dir_all(base.join("log")).unwrap();
        let user_hooks = json!({ "hooks": {
            "SessionStart": [{ "hooks": [{ "type": "command", "command": "mine" }] }],
            "PostToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "theirs" }] }],
        }});
        std::fs::write(base.join("hooks.json"), user_hooks.to_string()).unwrap();
        let homes = root.0.join("homes");
        let home = ThreadHome::prepare(&base, &homes, &hook()).unwrap();
        assert_eq!(home.dir.parent(), Some(homes.as_path()));
        for linked in [
            "auth.json",
            "installation_id",
            "plugins",
            "sessions",
            "archived_sessions",
            "generated_images",
        ] {
            let link = std::fs::read_link(home.dir.join(linked))
                .unwrap_or_else(|_| panic!("{linked} is linked"));
            assert_eq!(link, base.join(linked));
        }
        for left_out in ["state_5.sqlite", "state_5.sqlite-wal", "log"] {
            assert!(!home.dir.join(left_out).exists(), "{left_out}");
        }
        let config = home.dir.join("config.toml");
        assert!(std::fs::symlink_metadata(&config).unwrap().is_file());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "model = \"m\"\n");
        let hooks: Value =
            serde_json::from_str(&std::fs::read_to_string(home.dir.join("hooks.json")).unwrap())
                .unwrap();
        assert_eq!(
            hooks["hooks"]["SessionStart"],
            user_hooks["hooks"]["SessionStart"]
        );
        let post = hooks["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(post[0], user_hooks["hooks"]["PostToolUse"][0]);
        assert_eq!(
            post[1],
            json!({
                "matcher": "Bash|apply_patch",
                "hooks": [{
                    "type": "command",
                    "command": "'/Applications/Brigadier App.app/brigadierd' hook post-tool-use --codex",
                    "timeout": 10,
                }],
            })
        );
        assert_eq!(
            home.own_key(),
            format!("{}/hooks.json:post_tool_use:1:0", home.dir.display())
        );
        std::fs::write(base.join("config.toml"), "model = \"n\"\n").unwrap();
        std::fs::write(base.join("models_cache.json"), "{}").unwrap();
        let again = ThreadHome::prepare(&base, &homes, &hook()).unwrap();
        assert_eq!(again.dir, home.dir);
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "model = \"n\"\n");
        assert!(std::fs::read_link(home.dir.join("models_cache.json")).is_ok());
        let left: Vec<_> = std::fs::read_dir(&home.dir)
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(left.is_empty(), "no temp file left: {left:?}");
        // Another account's threads get a home of their own.
        let other = root.0.join("other");
        std::fs::create_dir_all(&other).unwrap();
        assert_ne!(
            ThreadHome::prepare(&other, &homes, &hook()).unwrap().dir,
            home.dir
        );
    }

    /// The trust Codex wrote into the home's copy for Brigadier's hook outlives the next
    /// launch's fresh copy, so a thread starting meanwhile keeps it; a user's hook has only the
    /// trust their config gives it now. With hooks switched off there's no home (#121).
    #[cfg(unix)]
    #[test]
    fn brigadiers_hook_trust_outlives_a_fresh_copy_of_the_config() {
        let root = scratch();
        let base = root.0.join("base");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("config.toml"), "model = \"m\"").unwrap();
        let user_hooks = json!({ "hooks": {
            "PostToolUse": [{ "hooks": [{ "type": "command", "command": "theirs" }] }],
        }});
        std::fs::write(base.join("hooks.json"), user_hooks.to_string()).unwrap();
        let homes = root.0.join("homes");
        let home = ThreadHome::prepare(&base, &homes, &hook()).unwrap();
        let config = home.dir.join("config.toml");
        let own = home.own_key();
        let theirs = format!("{}/hooks.json:post_tool_use:0:0", home.dir.display());
        let trusted = format!(
            "model = \"m\"\n\n[hooks.state.\"{own}\"]\ntrusted_hash = \"sha256:aa\"\n\n[hooks.state.\"{theirs}\"]\ntrusted_hash = \"sha256:bb\"\n"
        );
        std::fs::write(&config, &trusted).unwrap();
        // The user changed their config, and no longer trusts their hook.
        std::fs::write(base.join("config.toml"), "model = \"n\"").unwrap();
        let again = ThreadHome::prepare(&base, &homes, &hook()).unwrap();
        assert_eq!(again.own_key(), own);
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            format!("model = \"n\"\n\n[hooks.state.\"{own}\"]\ntrusted_hash = \"sha256:aa\"\n")
        );
        assert!(again.carried_trust().is_empty());
        std::fs::write(base.join("config.toml"), "[features]\nhooks = false\n").unwrap();
        assert!(ThreadHome::prepare(&base, &homes, &hook()).is_err());
        // Kept trust that wouldn't make one valid file with the user's is dropped.
        let clash = "hooks = { state = {} }\n";
        assert_eq!(with_kept_trust(clash, &trusted, &own), clash);
        assert_eq!(with_kept_trust("a = 1\n", "not = [toml", &own), "a = 1\n");
        assert!(hooks_on("") && hooks_on("[features]\nhooks = true\n"));
        assert!(!hooks_on("features.hooks = false\n"));
    }

    /// A relative `sqlite_home` in the user's config names a folder of the account's home: the
    /// copy names it from there, or Codex would take it from the thread's home, where the
    /// databases aren't (it wins over `CODEX_SQLITE_HOME`).
    #[cfg(unix)]
    #[test]
    fn a_relative_sqlite_home_stays_in_the_accounts_home() {
        let root = scratch();
        let base = root.0.join("base");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(
            base.join("config.toml"),
            "model = \"m\"\nsqlite_home = \"sqlite\" # mine\n",
        )
        .unwrap();
        let home = ThreadHome::prepare(&base, &root.0.join("homes"), &hook()).unwrap();
        assert_eq!(
            std::fs::read_to_string(home.dir.join("config.toml")).unwrap(),
            format!(
                "model = \"m\"\nsqlite_home = \"{}\" # mine\n",
                base.join("sqlite").display()
            )
        );
        for kept in [
            "sqlite_home = \"/data/codex\"\n",
            "sqlite_home = \"~/codex\"\n",
            "a = 1\n",
            "not = [toml",
        ] {
            assert_eq!(with_base_paths(kept, &base), kept);
        }
    }

    /// With no hooks of the user's, or a file that isn't hooks, Brigadier's is the only group.
    #[test]
    fn without_the_users_hooks_brigadiers_is_the_first_group() {
        for user in [None, Some("not json"), Some("{\"hooks\": 3}")] {
            let (text, index) = session_hooks(user, &hook());
            let file: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(index, 0, "{user:?}");
            assert_eq!(file["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
        }
    }

    /// A hook of the user's keeps its trust (and its being off) under its key in the session's
    /// home; other entries aren't carried.
    #[test]
    fn the_users_hook_trust_is_carried_to_the_session_homes_keys() {
        let base = Path::new("/Users/me/.codex");
        let home = Path::new("/data/codex-homes/s 1");
        let config = r#"
model = "m"

[hooks.state."/Users/me/.codex/hooks.json:post_tool_use:0:0"]
trusted_hash = "sha256:aa"

[hooks.state."/Users/me/.codex/hooks.json:stop:1:0"]
trusted_hash = "sha256:bb"
enabled = false

[hooks.state."/Users/me/.codex/config.toml:pre_tool_use:0:0"]
trusted_hash = "sha256:cc"

[hooks.state."/repo/.codex/hooks.json:stop:0:0"]
trusted_hash = "sha256:dd"
"#;
        let carried = carried_trust(config, base, home);
        assert_eq!(
            Value::Object(carried),
            json!({
                "/data/codex-homes/s 1/hooks.json:post_tool_use:0:0": { "trusted_hash": "sha256:aa" },
                "/data/codex-homes/s 1/hooks.json:stop:1:0": { "trusted_hash": "sha256:bb", "enabled": false },
                "/data/codex-homes/s 1/config.toml:pre_tool_use:0:0": { "trusted_hash": "sha256:cc" },
            })
        );
        assert!(carried_trust("not = [toml", base, home).is_empty());
        assert!(carried_trust("model = \"m\"\n", base, home).is_empty());
    }

    /// Only a login kept in `auth.json`, with hooks on, lets a thread have a home of its own.
    #[test]
    fn a_keyring_login_keeps_the_thread_on_the_users_home() {
        let root = scratch();
        let env = CliEnv::from_vars([(
            std::ffi::OsString::from("CODEX_HOME"),
            root.0.clone().into_os_string(),
        )]);
        // Never on Windows, where the login can't follow the thread there.
        assert_eq!(runs_thread_hook(&env), cfg!(unix), "no config: auth.json");
        for (config, runs) in [
            ("cli_auth_credentials_store = \"file\"\n", true),
            ("cli_auth_credentials_store = \"keyring\"\n", false),
            ("cli_auth_credentials_store = \"auto\"\n", false),
            ("[features]\nhooks = false\n", false),
        ] {
            std::fs::write(root.0.join("config.toml"), config).unwrap();
            assert_eq!(runs_thread_hook(&env), runs && cfg!(unix), "{config}");
        }
    }
}
