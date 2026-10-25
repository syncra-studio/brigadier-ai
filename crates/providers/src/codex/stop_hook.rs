//! A Codex thread's `Stop` hook (harness D49, superseding the Codex part of D46): the same
//! `brigadierd hook stop` a Claude thread runs ([`SessionSpec::stop_hook`]).
//!
//! Codex 0.164.0 runs a hook only once the user trusted it: `[hooks.state."<key>"]
//! trusted_hash` in their own `config.toml` (a trust passed with `-c` is ignored). So the hook
//! itself comes with the thread's own flags (`-c hooks.Stop=…`, Codex's session-flags layer),
//! and plain Codex sessions never see it; Brigadier writes only its trust, once, under the key
//! Codex gives a session-flags hook ([`KEY`]). The command names the daemon through
//! [`VAR`], set in the thread's environment, so every Brigadier build shares the one entry.
//!
//! The hash is Codex's own (`codex-rs/hooks/src/engine/discovery.rs` `hook_hash`, rust-v0.164.0):
//! SHA-256 of the hook's normalized identity as JSON with sorted keys, without spaces.
//! Checked against the real binary's `hooks/list` `currentHash`.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::claude::shell_quote;
use crate::model::OutputHook;

/// The key Codex gives the first handler of the first `Stop` group in its session flags.
pub(super) const KEY: &str = "/<session-flags>/config.toml:stop:0:0";

/// The thread's variable that holds the daemon's path, for the hook's command.
pub(super) const VAR: &str = "BRIGADIER_STOP_HOOK";

/// The command line Codex runs through the shell: the same for every Brigadier build.
fn command(hook: &OutputHook) -> String {
    std::iter::once(format!("\"${VAR}\""))
        .chain(hook.args.iter().map(|arg| shell_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Codex's normalized hook: no matcher for `Stop`, a timeout of at least a second.
fn handler(hook: &OutputHook) -> Value {
    json!({
        "type": "command",
        "command": command(hook),
        "timeout": hook.timeout_secs.max(1),
    })
}

/// The `-c` override that adds the hook to the thread's config.
pub(super) fn config(hook: &OutputHook) -> String {
    format!(
        "hooks.Stop={}",
        super::toml_value(&json!([{ "hooks": [handler(hook)] }]))
    )
}

/// The hash Codex trusts the hook by.
pub(super) fn hash(hook: &OutputHook) -> String {
    let mut handler = handler(hook);
    handler["async"] = Value::Bool(false);
    hash_of(json!({ "event_name": "stop", "hooks": [handler] }))
}

/// `sha256:<hex>` of `identity` as compact JSON with sorted keys (Codex's `version_for_toml`).
fn hash_of(mut identity: Value) -> String {
    identity.sort_all_objects();
    let digest = Sha256::digest(identity.to_string().as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn hook(args: &[&str], timeout_secs: u64) -> OutputHook {
        OutputHook {
            command: PathBuf::from("/Applications/Brigadier App.app/brigadierd"),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: Vec::new(),
            timeout_secs,
        }
    }

    #[test]
    fn the_hash_is_the_one_codex_computes() {
        // codex-cli 0.164.0 `hooks/list` for `command = "/bin/echo hi"`, `timeout = 5` in a
        // config.toml: currentHash.
        let identity = json!({
            "event_name": "stop",
            "hooks": [{ "type": "command", "command": "/bin/echo hi", "timeout": 5, "async": false }],
        });
        assert_eq!(
            hash_of(identity),
            "sha256:6970486fae0b2896a8b644de5827a2083935b46940a85fc3d1a2f40d690df200"
        );
        // The thread's hook, `"$BRIGADIER_STOP_HOOK" stop` with 10 s, ran trusted by this hash
        // on the real binary.
        let probe = OutputHook {
            args: vec!["stop".into()],
            timeout_secs: 10,
            ..hook(&[], 0)
        };
        let probe_hash = hash_of(json!({
            "event_name": "stop",
            "hooks": [{ "type": "command", "command": "\"$W30_PROBE_HOOK\" stop", "timeout": 10, "async": false }],
        }));
        assert_eq!(
            probe_hash,
            "sha256:ada0be2f4b70d00c208e587a2633b12494a80f1473a16a45ce6ae36079f0aee7"
        );
        assert_eq!(command(&probe), "\"$BRIGADIER_STOP_HOOK\" stop");
    }

    #[test]
    fn the_command_and_its_hash_do_not_name_the_build() {
        let one = hook(&["hook", "stop"], 30);
        let other = OutputHook {
            command: PathBuf::from("/tmp/dev/brigadierd"),
            ..one.clone()
        };
        assert_eq!(command(&one), "\"$BRIGADIER_STOP_HOOK\" hook stop");
        assert_eq!(hash(&one), hash(&other));
        assert_eq!(config(&one), config(&other));
        // serde_json keeps insertion order in some builds: the pieces, in any order.
        let given = config(&one);
        assert!(given.starts_with(r#"hooks.Stop=[{"hooks" = [{"#), "{given}");
        for piece in [
            r#""command" = "\"$BRIGADIER_STOP_HOOK\" hook stop""#,
            r#""timeout" = 30"#,
            r#""type" = "command""#,
        ] {
            assert!(given.contains(piece), "{given}");
        }
        // What Codex hashes follows what the config says.
        assert_ne!(hash(&one), hash(&hook(&["hook", "stop"], 31)));
    }
}
